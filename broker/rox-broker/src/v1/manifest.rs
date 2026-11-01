//! Verifying a plugin's `buwiz://manifest` (spec 5; `v1/manifest.mjs`).
//!
//! Nothing a plugin says is trusted until its manifest passes, in the
//! spec's order (5.2): I-JSON parse, protocol and version, trust store,
//! publisher signature over JCS, then every field's grammar and limit
//! (5.3-5.9, 6.6, Appendix A). A failure is a [`V1Error`] with the spec's
//! code and the field's path in the message. The same module answers the
//! install review lines (13.1 step 3), the permissions digest and the props
//! check of 6.6.
//!
//! Where Node would throw a TypeError on a malformed manifest (a tool name
//! that is a list, say), this refuses it as `manifest_invalid` instead.

use serde_json::{json, Map, Value};

use super::params::schema_problem;
use super::trust::Trust;
use super::{canonicalize, ed_verify, get, is_v1, kind_of, safe_int, same, shown, signed_bytes, tpl, V1Error};
use crate::js;

// 2.1: packages (plugin.json's signed buwiz block) and accounts
// (buwiz.account.set/clear). 2.0: the buwiz names (buwiz.* reserved tools, buwiz:// resources,
// buwiz-call/buwiz-session, buwiz/<key> _meta); 1.x plugins speak rox
pub const HOST_PROTOCOL: &str = "2.1.0";
pub const APP_MIME: &str = "text/x-nsh-tsx";
pub const MANIFEST_URI: &str = "buwiz://manifest";

/// Appendix A, the limits a manifest is held to.
pub mod limits {
    pub const MANIFEST_BYTES: usize = 256 * 1024;
    pub const PLUGIN_ID: usize = 255;
    pub const VERSION: usize = 32;
    pub const TITLE: usize = 60;
    pub const DESCRIPTION: usize = 300;
    pub const COMMANDS: usize = 32;
    pub const SCOPES: usize = 16;
    pub const ROUTINES: usize = 8;
    pub const APPS: usize = 16;
    pub const COMMAND_DESCRIPTION: usize = 200;
    pub const COMMAND_ARGS: usize = 120;
    pub const SCOPE_DESCRIPTION: usize = 120;
    pub const SCOPE_HINT: usize = 1500;
    pub const ROUTINE_NAME: usize = 80;
    pub const ROUTINE_DESCRIPTION: usize = 300;
    pub const ROUTINE_PROMPT: usize = 4000;
    pub const PUBLISHER_NAME: usize = 80;
    pub const PERM_TOOLS: usize = 64;
    pub const OPEN_APPS: usize = 16;
    pub const OPEN_EXTERNAL: usize = 8;
    pub const ATTACH: usize = 16;
    pub const PICK_EXTENSIONS: usize = 16;
    pub const PICK_MAX_BYTES: u64 = 50 * 1024 * 1024;
    pub const STORAGE_QUOTA: u64 = 10 * 1024 * 1024;
    pub const STORAGE_DEFAULT: u64 = 1024 * 1024;
    pub const PREVIEW_MAX_BYTES: u64 = 8 * 1024 * 1024;
    pub const PROPS_BYTES: usize = 8 * 1024;
    pub const PROPS_DEPTH: usize = 8;
    pub const PROPS_ARRAY: usize = 100;
    pub const SCHEMA_PROPERTIES: usize = 16;
    pub const SCHEMA_MAX_LENGTH: u64 = 512;
    pub const INLINE_WIDTH: (u64, u64) = (240, 960);
    pub const INLINE_HEIGHT: (u64, u64) = (96, 800);
    pub const EVERY_S: (u64, u64) = (60, 86400);
    pub const READS: usize = 4;
    pub const READS_TOOLS: usize = 8;
    pub const WRITES: usize = 8;
}

use limits as L;

/// The limits as Node's `LIMITS` object names them.
pub fn limits_json() -> Value {
    json!({
        "manifestBytes": L::MANIFEST_BYTES, "pluginId": L::PLUGIN_ID, "version": L::VERSION, "title": L::TITLE,
        "description": L::DESCRIPTION, "commands": L::COMMANDS, "scopes": L::SCOPES, "routines": L::ROUTINES, "apps": L::APPS,
        "commandDescription": L::COMMAND_DESCRIPTION, "commandArgs": L::COMMAND_ARGS, "scopeDescription": L::SCOPE_DESCRIPTION,
        "scopeHint": L::SCOPE_HINT, "routineName": L::ROUTINE_NAME, "routineDescription": L::ROUTINE_DESCRIPTION,
        "routinePrompt": L::ROUTINE_PROMPT, "publisherName": L::PUBLISHER_NAME, "permTools": L::PERM_TOOLS,
        "openApps": L::OPEN_APPS, "openExternal": L::OPEN_EXTERNAL, "attach": L::ATTACH, "pickExtensions": L::PICK_EXTENSIONS,
        "pickMaxBytes": L::PICK_MAX_BYTES, "storageQuota": L::STORAGE_QUOTA, "storageDefault": L::STORAGE_DEFAULT,
        "previewMaxBytes": L::PREVIEW_MAX_BYTES, "propsBytes": L::PROPS_BYTES, "propsDepth": L::PROPS_DEPTH,
        "propsArray": L::PROPS_ARRAY, "schemaProperties": L::SCHEMA_PROPERTIES, "schemaMaxLength": L::SCHEMA_MAX_LENGTH,
        "inlineWidth": [L::INLINE_WIDTH.0, L::INLINE_WIDTH.1], "inlineHeight": [L::INLINE_HEIGHT.0, L::INLINE_HEIGHT.1],
        "everyS": [L::EVERY_S.0, L::EVERY_S.1], "reads": L::READS, "readsTools": L::READS_TOOLS, "writes": L::WRITES,
    })
}

// the app's bundle id (com.buwiz.bot), itself and under it
const RESERVED_ID_PREFIXES: &[&str] = &["buwiz.", "com.buwiz.bot.", "com.buwiz.bot"];
const RESERVED_SHORTS: &[&str] = &["buwiz", "app", "chat", "mcp", "nsh", "all"];
const RESERVED_COMMANDS: &[&str] = &["status"];

type Res<T = ()> = Result<T, V1Error>;

/// sha256(JCS(permissions)), lowercase hex; absent permissions are `{}`.
pub fn perms_digest(perms: Option<&Value>) -> Result<String, String> {
    signed_bytes(perms.unwrap_or(&json!({}))).map(|b| rox_v1::keys::sha256_hex(&b))
}

/// The manifest's identity hash: sha256 of its JCS bytes (5.2 step 7).
pub fn manifest_hash(m: &Value) -> Result<String, String> {
    signed_bytes(m).map(|b| rox_v1::keys::sha256_hex(&b))
}

// ── semver ───────────────────────────────────────────────────────────────

/// `0|[1-9]\d*`.
fn numeric(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'))
}

/// A semver 2.0 string's major, minor, patch and prerelease, or `None`.
fn semver(s: &str) -> Option<([&str; 3], Option<&str>)> {
    let (rest, build) = match s.split_once('+') {
        Some((r, b)) => (r, Some(b)),
        None => (s, None),
    };
    let ident = |id: &str| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if build.is_some_and(|b| !b.split('.').all(ident)) {
        return None;
    }
    let (core, pre) = match rest.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (rest, None),
    };
    // a prerelease id is numeric without a leading zero, or holds a non-digit
    if pre.is_some_and(|p| !p.split('.').all(|id| ident(id) && (numeric(id) || !id.bytes().all(|b| b.is_ascii_digit())))) {
        return None;
    }
    let parts: Vec<&str> = core.split('.').collect();
    let [a, b, c] = parts[..] else { return None };
    [a, b, c].iter().all(|p| numeric(p)).then_some(([a, b, c], pre))
}

pub fn is_semver(v: Option<&Value>) -> bool {
    v.and_then(Value::as_str).is_some_and(|s| semver(s).is_some())
}

/// -1, 0 or 1 comparing two semver strings by precedence (semver 2.0 11);
/// `None` when either is not semver.
pub fn compare_semver(a: &str, b: &str) -> Option<i32> {
    let ((ca, pa), (cb, pb)) = (semver(a)?, semver(b)?);
    let num = |s: &str| s.parse::<f64>().unwrap_or(f64::NAN);
    for i in 0..3 {
        if num(ca[i]) != num(cb[i]) {
            return Some(if num(ca[i]) < num(cb[i]) { -1 } else { 1 });
        }
    }
    let (Some(pa), Some(pb)) = (pa.filter(|p| !p.is_empty()), pb.filter(|p| !p.is_empty())) else {
        return Some(match (pa, pb) {
            (x, y) if x == y => 0,
            (Some(_), _) => -1,
            _ => 1,
        });
    };
    let (xa, xb): (Vec<&str>, Vec<&str>) = (pa.split('.').collect(), pb.split('.').collect());
    for i in 0..xa.len().max(xb.len()) {
        let (Some(x), Some(y)) = (xa.get(i), xb.get(i)) else { return Some(if xa.get(i).is_none() { -1 } else { 1 }) };
        if x == y {
            continue;
        }
        let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
        let (na, nb) = (digits(x), digits(y));
        if na && nb {
            return Some(if num(x) < num(y) { -1 } else { 1 });
        }
        if na != nb {
            return Some(if na { -1 } else { 1 });
        }
        return Some(if x.encode_utf16().lt(y.encode_utf16()) { -1 } else { 1 });
    }
    Some(0)
}

// ── field checks ─────────────────────────────────────────────────────────

fn invalid(path: &str, why: impl std::fmt::Display) -> V1Error {
    V1Error::new("manifest_invalid", format!("{path}: {why}"))
}

fn text(v: Option<&Value>, path: &str, min: usize, max: Option<usize>, required: bool) -> Res {
    if v.is_none() && !required {
        return Ok(());
    }
    let Some(Value::String(s)) = v else { return Err(invalid(path, "must be a string")) };
    let n = s.chars().count();
    if n < min {
        return Err(invalid(path, if min == 1 { "must not be empty".to_owned() } else { format!("must be at least {min} characters") }));
    }
    match max {
        Some(max) if n > max => Err(invalid(path, format!("is over {max} characters"))),
        _ => Ok(()),
    }
}

fn name(v: Option<&Value>, kind: &str, path: &str) -> Res {
    if is_v1(kind, v) {
        Ok(())
    } else {
        Err(invalid(path, format!("{} breaks the {kind} grammar", shown(v, 80))))
    }
}

fn list<'a>(v: Option<&'a Value>, path: &str, max: usize, required: bool) -> Res<&'a [Value]> {
    match v {
        None if !required => Ok(&[]),
        Some(Value::Array(items)) if items.len() > max => Err(invalid(path, format!("has {} entries; at most {max}", items.len()))),
        Some(Value::Array(items)) => Ok(items),
        _ => Err(invalid(path, "must be an array")),
    }
}

fn obj<'a>(v: Option<&'a Value>, path: &str) -> Res<&'a Map<String, Value>> {
    match v {
        Some(Value::Object(o)) => Ok(o),
        _ => Err(invalid(path, "must be an object")),
    }
}

/// Objects that carry an access grant refuse unknown keys (2.2: fail closed).
fn only_keys(o: &Map<String, Value>, allowed: &[&str], path: &str) -> Res {
    match js::ordered(o).into_iter().find(|(k, _)| !allowed.contains(&k.as_str())) {
        Some((k, _)) => Err(invalid(&format!("{path}.{k}"), "is not a known key here (refused: unknown keys in access grants fail closed)")),
        None => Ok(()),
    }
}

fn int(v: Option<&Value>, path: &str, lo: f64, hi: f64) -> Res {
    let Some(n) = safe_int(v) else { return Err(invalid(path, "must be an integer")) };
    if n < lo || n > hi {
        return Err(invalid(path, format!("must be {} to {}", js::number(lo), js::number(hi))));
    }
    Ok(())
}

fn unique(items: &[Value], key: &str, path: &str) -> Res {
    for (i, x) in items.iter().enumerate() {
        let v = get(x, key);
        if items[..i].iter().any(|y| same(get(y, key), v)) {
            return Err(invalid(&format!("{path}[{i}].{key}"), format!("{} is not unique", shown(v, usize::MAX))));
        }
    }
    Ok(())
}

fn own_tool(v: Option<&Value>, short: &str, path: &str) -> Res {
    name(v, "tool", path)?;
    match v {
        Some(Value::String(t)) if t.starts_with(&format!("{short}_")) => Ok(()),
        Some(Value::String(t)) => Err(invalid(path, format!("tool {t} must start with {short}_"))),
        _ => Err(invalid(path, "must be a string")),
    }
}

fn boolean(o: &Map<String, Value>, key: &str, path: &str) -> Res {
    match o.get(key) {
        Some(v) if !v.is_boolean() => Err(invalid(&format!("{path}.{key}"), "must be a boolean")),
        _ => Ok(()),
    }
}

// ── props schema subset (6.6) ────────────────────────────────────────────

const SCHEMA_KEYS: &[&str] = &["type", "properties", "required", "items", "enum", "maxLength", "minimum", "maximum"];
const SCHEMA_TYPES: &[&str] = &["object", "array", "string", "number", "integer", "boolean", "null"];

fn schema(s: Option<&Value>, path: &str, depth: usize) -> Res {
    let o = obj(s, path)?;
    if depth > L::PROPS_DEPTH {
        return Err(invalid(path, format!("nests deeper than {}", L::PROPS_DEPTH)));
    }
    for (k, _) in js::ordered(o) {
        if !SCHEMA_KEYS.contains(&k.as_str()) {
            return Err(invalid(&format!("{path}.{k}"), format!("is outside the props schema subset ({})", SCHEMA_KEYS.join(", "))));
        }
    }
    if o.get("type").is_some_and(|t| !t.as_str().is_some_and(|t| SCHEMA_TYPES.contains(&t))) {
        return Err(invalid(&format!("{path}.type"), format!("must be one of {}", SCHEMA_TYPES.join(", "))));
    }
    if o.contains_key("properties") {
        let props = obj(o.get("properties"), &format!("{path}.properties"))?;
        if props.len() > L::SCHEMA_PROPERTIES {
            return Err(invalid(&format!("{path}.properties"), format!("has {} properties; at most {}", props.len(), L::SCHEMA_PROPERTIES)));
        }
        for (k, sub) in js::ordered(props) {
            name(Some(&Value::String(k.clone())), "propKey", &format!("{path}.properties"))?;
            schema(Some(sub), &format!("{path}.properties.{k}"), depth + 1)?;
        }
    }
    if o.contains_key("required") {
        let props = o.get("properties").and_then(Value::as_object);
        for (i, r) in list(o.get("required"), &format!("{path}.required"), L::SCHEMA_PROPERTIES, false)?.iter().enumerate() {
            if !r.as_str().is_some_and(|r| props.is_some_and(|p| p.contains_key(r))) {
                return Err(invalid(&format!("{path}.required[{i}]"), "must name one of the properties"));
            }
        }
    }
    if o.contains_key("items") {
        schema(o.get("items"), &format!("{path}.items"), depth + 1)?;
    }
    if o.contains_key("enum") {
        for (i, e) in list(o.get("enum"), &format!("{path}.enum"), L::PROPS_ARRAY, true)?.iter().enumerate() {
            if e.is_array() || e.is_object() {
                return Err(invalid(&format!("{path}.enum[{i}]"), "must be a scalar"));
            }
        }
    }
    if o.contains_key("maxLength") {
        int(o.get("maxLength"), &format!("{path}.maxLength"), 0.0, L::SCHEMA_MAX_LENGTH as f64)?;
    }
    for k in ["minimum", "maximum"] {
        if o.contains_key(k) && safe_int(o.get(k)).is_none() {
            return Err(invalid(&format!("{path}.{k}"), "must be an integer"));
        }
    }
    Ok(())
}

// ── the sections ─────────────────────────────────────────────────────────

fn publisher(p: Option<&Value>) -> Res {
    let o = obj(p, "publisher")?;
    if !o.get("id").and_then(Value::as_str).is_some_and(|id| super::grammar_ok("publisherId", id)) {
        return Err(invalid("publisher.id", "breaks the publisher id grammar"));
    }
    name(o.get("key_id"), "keyId", "publisher.key_id")?;
    text(o.get("name"), "publisher.name", 1, Some(L::PUBLISHER_NAME), true)
}

fn commands(m: &Map<String, Value>, short: &str) -> Res {
    let cs = list(m.get("commands"), "commands", L::COMMANDS, false)?;
    for (i, c) in cs.iter().enumerate() {
        let at = format!("commands[{i}]");
        let o = obj(Some(c), &at)?;
        name(o.get("name"), "command", &format!("{at}.name"))?;
        if let Some(n) = o.get("name").and_then(Value::as_str).filter(|n| RESERVED_COMMANDS.contains(n)) {
            return Err(invalid(&format!("{at}.name"), format!("/{n} is a built-in command")));
        }
        text(o.get("description"), &format!("{at}.description"), 0, Some(L::COMMAND_DESCRIPTION), true)?;
        text(o.get("args"), &format!("{at}.args"), 0, Some(L::COMMAND_ARGS), false)?;
        own_tool(o.get("resolve"), short, &format!("{at}.resolve"))?;
    }
    unique(cs, "name", "commands")
}

fn scopes(m: &Map<String, Value>, short: &str) -> Res {
    let ss = list(m.get("scopes"), "scopes", L::SCOPES, false)?;
    for (i, s) in ss.iter().enumerate() {
        let at = format!("scopes[{i}]");
        let o = obj(Some(s), &at)?;
        name(o.get("name"), "scope", &format!("{at}.name"))?;
        text(o.get("description"), &format!("{at}.description"), 0, Some(L::SCOPE_DESCRIPTION), true)?;
        own_tool(o.get("resolve"), short, &format!("{at}.resolve"))?;
        boolean(o, "attach", &at)?;
        text(o.get("hint"), &format!("{at}.hint"), 0, Some(L::SCOPE_HINT), false)?;
    }
    unique(ss, "name", "scopes")
}

fn routines(m: &Map<String, Value>, short: &str) -> Res {
    let rs = list(m.get("routines"), "routines", L::ROUTINES, false)?;
    for (i, r) in rs.iter().enumerate() {
        let at = format!("routines[{i}]");
        let o = obj(Some(r), &at)?;
        // a routine carries access grants (`reads`, `writes`): unknown keys,
        // v0's `needs` among them, are refused (2.2, 5.9)
        only_keys(o, &["id", "name", "description", "every_s", "query", "prompt", "reads", "writes", "params"], &at)?;
        name(o.get("id"), "routineId", &format!("{at}.id"))?;
        text(o.get("name"), &format!("{at}.name"), 1, Some(L::ROUTINE_NAME), true)?;
        text(o.get("description"), &format!("{at}.description"), 0, Some(L::ROUTINE_DESCRIPTION), true)?;
        int(o.get("every_s"), &format!("{at}.every_s"), L::EVERY_S.0 as f64, L::EVERY_S.1 as f64)?;
        let q = obj(o.get("query"), &format!("{at}.query"))?;
        only_keys(q, &["tool", "field", "args"], &format!("{at}.query"))?;
        own_tool(q.get("tool"), short, &format!("{at}.query.tool"))?;
        text(q.get("field"), &format!("{at}.query.field"), 1, Some(64), true)?;
        if q.contains_key("args") {
            obj(q.get("args"), &format!("{at}.query.args"))?;
        }
        // `params`: what one instance runs with (a small JSON Schema), passed
        // to the query tool beside query.args, so no name may be in both
        if let Some(params) = o.get("params") {
            let why = schema_problem(params);
            if !why.is_empty() {
                return Err(invalid(&format!("{at}.params"), why));
            }
            let args = q.get("args").and_then(Value::as_object);
            let props = get(params, "properties").and_then(Value::as_object).expect("checked by schema_problem");
            if let Some((both, _)) = js::ordered(props).into_iter().find(|(k, _)| args.is_some_and(|a| a.contains_key(*k))) {
                return Err(invalid(&format!("{at}.params"), format!("{both} is in query.args too")));
            }
        }
        text(o.get("prompt"), &format!("{at}.prompt"), 1, Some(L::ROUTINE_PROMPT), true)?;
        let mut tools = 0;
        for (j, x) in list(o.get("reads"), &format!("{at}.reads"), L::READS, false)?.iter().enumerate() {
            let xo = obj(Some(x), &format!("{at}.reads[{j}]"))?;
            only_keys(xo, &["plugin", "tools"], &format!("{at}.reads[{j}]"))?;
            name(xo.get("plugin"), "pluginId", &format!("{at}.reads[{j}].plugin"))?;
            let ts = list(xo.get("tools"), &format!("{at}.reads[{j}].tools"), L::READS_TOOLS, true)?;
            for (k, t) in ts.iter().enumerate() {
                name(Some(t), "tool", &format!("{at}.reads[{j}].tools[{k}]"))?;
            }
            tools += ts.len();
        }
        if tools > L::READS_TOOLS {
            return Err(invalid(&format!("{at}.reads"), format!("names {tools} tools; at most {}", L::READS_TOOLS)));
        }
        // `writes`: the plugin's own tools that change data, which the
        // routine's turn may call without asking (5.9)
        let writes = list(o.get("writes"), &format!("{at}.writes"), L::WRITES, false)?;
        for (k, t) in writes.iter().enumerate() {
            if is_v1("reservedTool", Some(t)) {
                return Err(invalid(&format!("{at}.writes[{k}]"), format!("{} is reserved for Buwiz Bot", tpl(Some(t)))));
            }
            own_tool(Some(t), short, &format!("{at}.writes[{k}]"))?;
            if writes.iter().position(|w| same(Some(w), Some(t))) != Some(k) {
                return Err(invalid(&format!("{at}.writes[{k}]"), format!("{} is listed twice", tpl(Some(t)))));
            }
        }
    }
    unique(rs, "id", "routines")
}

/// `new URL(o).protocol === "https:" && new URL(o).origin === o`.
fn https_origin(o: &Value) -> bool {
    let Value::String(s) = o else { return false };
    url::Url::parse(s).is_ok_and(|u| u.scheme() == "https" && u.origin().ascii_serialization() == *s)
}

fn permissions(p: Option<&Value>, m: &Map<String, Value>, short: &str, at: &str) -> Res {
    let o = obj(p, at)?;
    only_keys(o, &["tools", "open", "attach", "pick_file", "storage", "copy", "preview"], at)?;
    for (i, t) in list(o.get("tools"), &format!("{at}.tools"), L::PERM_TOOLS, false)?.iter().enumerate() {
        if is_v1("reservedTool", Some(t)) {
            return Err(invalid(&format!("{at}.tools[{i}]"), format!("{} is reserved for Buwiz Bot", tpl(Some(t)))));
        }
        own_tool(Some(t), short, &format!("{at}.tools[{i}]"))?;
    }
    if o.contains_key("open") {
        let open = obj(o.get("open"), &format!("{at}.open"))?;
        only_keys(open, &["apps", "external"], &format!("{at}.open"))?;
        let ids: Vec<Option<&Value>> = m.get("apps").and_then(Value::as_array).into_iter().flatten().map(|a| get(a, "id")).collect();
        for (i, a) in list(open.get("apps"), &format!("{at}.open.apps"), L::OPEN_APPS, false)?.iter().enumerate() {
            if !ids.iter().any(|id| same(*id, Some(a))) {
                return Err(invalid(&format!("{at}.open.apps[{i}]"), format!("{} is not an app of this plugin", shown(Some(a), 60))));
            }
        }
        for (i, x) in list(open.get("external"), &format!("{at}.open.external"), L::OPEN_EXTERNAL, false)?.iter().enumerate() {
            if !https_origin(x) {
                return Err(invalid(&format!("{at}.open.external[{i}]"), "must be an https origin (https://host[:port], no path)"));
            }
        }
    }
    for (i, s) in list(o.get("attach"), &format!("{at}.attach"), L::ATTACH, false)?.iter().enumerate() {
        let scope = m.get("scopes").and_then(Value::as_array).into_iter().flatten().find(|x| same(get(x, "name"), Some(s)));
        if !scope.and_then(|x| get(x, "attach")).is_some_and(js::truthy) {
            return Err(invalid(&format!("{at}.attach[{i}]"), format!("{} is not a scope with attach: true", shown(Some(s), 60))));
        }
    }
    if o.contains_key("pick_file") {
        let at = format!("{at}.pick_file");
        let f = obj(o.get("pick_file"), &at)?;
        only_keys(f, &["extensions", "max_bytes", "multiple"], &at)?;
        for (i, e) in list(f.get("extensions"), &format!("{at}.extensions"), L::PICK_EXTENSIONS, false)?.iter().enumerate() {
            // ^\.[a-z0-9]{1,16}$
            let ok = e.as_str().and_then(|e| e.strip_prefix('.')).is_some_and(|x| (1..=16).contains(&x.len()) && x.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
            if !ok {
                return Err(invalid(&format!("{at}.extensions[{i}]"), "must be like .xlsx"));
            }
        }
        int(f.get("max_bytes"), &format!("{at}.max_bytes"), 1.0, L::PICK_MAX_BYTES as f64)?;
        boolean(f, "multiple", &at)?;
    }
    if o.contains_key("storage") {
        let st = obj(o.get("storage"), &format!("{at}.storage"))?;
        only_keys(st, &["quota_bytes"], &format!("{at}.storage"))?;
        if st.contains_key("quota_bytes") {
            int(st.get("quota_bytes"), &format!("{at}.storage.quota_bytes"), 1.0, L::STORAGE_QUOTA as f64)?;
        }
    }
    boolean(o, "copy", at)?;
    if o.contains_key("preview") {
        let pv = obj(o.get("preview"), &format!("{at}.preview"))?;
        only_keys(pv, &["max_bytes"], &format!("{at}.preview"))?;
        if pv.contains_key("max_bytes") {
            int(pv.get("max_bytes"), &format!("{at}.preview.max_bytes"), 1.0, L::PREVIEW_MAX_BYTES as f64)?;
        }
    }
    Ok(())
}

fn apps(m: &Map<String, Value>, short: &str) -> Res {
    let r#as = list(m.get("apps"), "apps", L::APPS, false)?;
    for (i, a) in r#as.iter().enumerate() {
        let at = format!("apps[{i}]");
        let o = obj(Some(a), &at)?;
        name(o.get("id"), "appId", &format!("{at}.id"))?;
        text(o.get("title"), &format!("{at}.title"), 1, Some(L::TITLE), true)?;
        text(o.get("description"), &format!("{at}.description"), 0, Some(L::DESCRIPTION), true)?;
        // a screen is TSX (entry, mime, sdk, fingerprint) or native (a host
        // screen of a first-party plugin, `verify`), never both
        if o.contains_key("native") {
            if ["entry", "mime", "sdk", "fingerprint"].iter().any(|k| o.contains_key(*k)) {
                return Err(invalid(&at, "is a TSX screen (entry) or a native screen (native), not both"));
            }
            let n = obj(o.get("native"), &format!("{at}.native"))?;
            only_keys(n, &["screen", "abi"], &format!("{at}.native"))?;
            // ^[a-z0-9._-]{1,64}$
            let screen_ok = n.get("screen").and_then(Value::as_str).is_some_and(|s| (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b)));
            if !screen_ok {
                return Err(invalid(&format!("{at}.native.screen"), "must be 1 to 64 characters of a-z, 0-9, '.', '_' and '-'"));
            }
            if safe_int(n.get("abi")) != Some(1.0) {
                return Err(invalid(&format!("{at}.native.abi"), "must be 1"));
            }
        } else {
            if !o.contains_key("entry") {
                return Err(invalid(&at, "needs entry (a TSX screen) or native (a native screen)"));
            }
            let entry = format!("buwiz-app://{}", tpl(o.get("id")));
            if o.get("entry").and_then(Value::as_str) != Some(entry.as_str()) {
                return Err(invalid(&format!("{at}.entry"), format!("must be {entry}")));
            }
            if o.get("mime").and_then(Value::as_str) != Some(APP_MIME) {
                return Err(invalid(&format!("{at}.mime"), format!("must be {APP_MIME}")));
            }
            if o.get("sdk").and_then(Value::as_str) != Some("1") {
                return Err(invalid(&format!("{at}.sdk"), "must be \"1\""));
            }
            // ^sha256:[0-9a-f]{64}$
            let fp_ok = o.get("fingerprint").and_then(Value::as_str).and_then(|f| f.strip_prefix("sha256:")).is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
            if !fp_ok {
                return Err(invalid(&format!("{at}.fingerprint"), "must be sha256:<64 lowercase hex>"));
            }
        }
        if o.contains_key("props") {
            schema(o.get("props"), &format!("{at}.props"), 1)?;
            if o.get("props").and_then(|p| get(p, "type")).and_then(Value::as_str) != Some("object") {
                return Err(invalid(&format!("{at}.props.type"), "must be \"object\""));
            }
        }
        let place = list(o.get("placement"), &format!("{at}.placement"), 2, true)?;
        let known = place.iter().all(|p| p == "full" || p == "inline");
        if place.is_empty() || !known || (place.len() == 2 && place[0] == place[1]) {
            return Err(invalid(&format!("{at}.placement"), "must be [\"full\"], [\"inline\"] or [\"full\",\"inline\"]"));
        }
        if place.iter().any(|p| p == "inline") {
            let inl = obj(o.get("inline"), &format!("{at}.inline"))?;
            let (w, h) = (L::INLINE_WIDTH, L::INLINE_HEIGHT);
            int(inl.get("width"), &format!("{at}.inline.width"), w.0 as f64, w.1 as f64)?;
            int(inl.get("height"), &format!("{at}.inline.height"), h.0 as f64, h.1 as f64)?;
            let width = safe_int(inl.get("width")).unwrap_or_default();
            let height = safe_int(inl.get("height")).unwrap_or_default();
            if inl.contains_key("min_width") {
                int(inl.get("min_width"), &format!("{at}.inline.min_width"), w.0 as f64, width)?;
            }
            // buwiz.fit's range (6.5): min_height <= height <= max_height
            if inl.contains_key("min_height") {
                int(inl.get("min_height"), &format!("{at}.inline.min_height"), h.0 as f64, height)?;
            }
            if inl.contains_key("max_height") {
                int(inl.get("max_height"), &format!("{at}.inline.max_height"), height, h.1 as f64)?;
            }
            boolean(inl, "resizable", &format!("{at}.inline"))?;
            // the card holds its placeholder until the app's buwiz.ready()
            // (an optional addition under 1.0: older hosts ignore it)
            boolean(inl, "wait_ready", &format!("{at}.inline"))?;
        }
        if o.contains_key("permissions") {
            permissions(o.get("permissions"), m, short, &format!("{at}.permissions"))?;
        }
    }
    unique(r#as, "id", "apps")
}

fn fields(m: &Map<String, Value>) -> Res {
    name(m.get("id"), "pluginId", "id")?;
    if let Some(id) = m.get("id").and_then(Value::as_str).filter(|id| RESERVED_ID_PREFIXES.iter().any(|p| id.starts_with(p))) {
        return Err(invalid("id", format!("{id} uses a prefix reserved for Buwiz Bot")));
    }
    name(m.get("short"), "short", "short")?;
    if let Some(s) = m.get("short").and_then(Value::as_str).filter(|s| RESERVED_SHORTS.contains(s)) {
        return Err(invalid("short", format!("{s} is reserved")));
    }
    text(m.get("title"), "title", 1, Some(L::TITLE), true)?;
    text(m.get("description"), "description", 0, Some(L::DESCRIPTION), false)?;
    text(m.get("icon"), "icon", 1, Some(2048), false)?;
    let short = tpl(m.get("short"));
    commands(m, &short)?;
    scopes(m, &short)?;
    routines(m, &short)?;
    apps(m, &short)
}

// ── verification (5.2) ───────────────────────────────────────────────────

fn protocol_of(m: &Map<String, Value>, host: &str) -> Res {
    let Some(protocol) = m.get("protocol").and_then(Value::as_str).filter(|p| semver(p).is_some()) else {
        return Err(invalid("protocol", "must be a semver string"));
    };
    let two = |s: &str| {
        let mut parts = s.split('.').map(|p| p.parse::<f64>().unwrap_or(f64::NAN));
        (parts.next().unwrap_or(f64::NAN), parts.next().unwrap_or(f64::NAN))
    };
    let ((maj, min), (hmaj, hmin)) = (two(protocol), two(host));
    if maj < hmaj {
        return Err(V1Error::new("protocol_unsupported", format!("the plugin implements protocol {protocol}: this plugin is built for an older protocol; update it")));
    }
    if maj != hmaj || min > hmin {
        let n = js::number;
        return Err(V1Error::new(
            "protocol_unsupported",
            format!("the plugin implements protocol {protocol}; this Buwiz Bot supports {}.{}, so it needs a Buwiz Bot that supports {}.{}", n(hmaj), n(hmin), n(maj), n(min)),
        ));
    }
    let version_ok = m.get("version").and_then(Value::as_str).is_some_and(|v| semver(v).is_some() && v.encode_utf16().count() <= L::VERSION);
    if !version_ok {
        return Err(invalid("version", format!("must be semver, at most {} characters", L::VERSION)));
    }
    Ok(())
}

fn signature_of(m: &Map<String, Value>, trust: &Trust) -> Res {
    publisher(m.get("publisher"))?;
    let p = m.get("publisher");
    let key_id_v = p.and_then(|p| get(p, "key_id"));
    let key_id = tpl(key_id_v);
    let Some(key) = key_id_v.and_then(Value::as_str).and_then(|k| trust.publishers.get(k)) else {
        return Err(invalid("publisher.key_id", format!("{key_id} is not a trusted publisher key")));
    };
    if !key.publisher.is_empty() && p.and_then(|p| get(p, "id")).and_then(Value::as_str) != Some(key.publisher.as_str()) {
        return Err(invalid("publisher.id", format!("key {key_id} belongs to {}", key.publisher)));
    }
    if !m.get("id").and_then(Value::as_str).is_some_and(|id| trust.is_approved(id, &key_id)) {
        return Err(invalid("id", format!("{} is not on the approved list for key {key_id}", js::slice_utf16(&tpl(m.get("id")), 80))));
    }
    let s = obj(m.get("signature"), "signature")?;
    if s.get("alg").and_then(Value::as_str) != Some("ed25519") {
        return Err(invalid("signature.alg", "must be \"ed25519\""));
    }
    if !same(s.get("key_id"), key_id_v) {
        return Err(invalid("signature.key_id", "must equal publisher.key_id"));
    }
    // ^[A-Za-z0-9_-]{86}$
    let Some(value) = s.get("value").and_then(Value::as_str).filter(|v| v.len() == 86 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')) else {
        return Err(invalid("signature.value", "must be 64 bytes of base64url"));
    };
    let mut unsigned = m.clone();
    unsigned.shift_remove("signature");
    let bytes = signed_bytes(&Value::Object(unsigned)).map_err(|e| invalid("manifest", e))?;
    if !ed_verify(&key.public_key, &bytes, value) {
        return Err(invalid("signature", "does not verify with the publisher key"));
    }
    Ok(())
}

/// A manifest that passed: the parsed value and its identity hash.
#[derive(Debug, Clone, PartialEq)]
pub struct Verified {
    pub manifest: Value,
    pub sha256: String,
}

/// Verifies manifest text against `trust` (spec 5.2 steps 1-5).
pub fn verify(text: &str, trust: &Trust, host: &str) -> Res<Verified> {
    if text.len() > L::MANIFEST_BYTES {
        return Err(invalid("manifest", format!("is over {} bytes", L::MANIFEST_BYTES)));
    }
    let m = rox_v1::jcs::parse_ijson(text).map_err(|e| invalid("manifest", format!("is not I-JSON: {e}")))?;
    let Value::Object(o) = &m else { return Err(invalid("manifest", format!("must be a JSON object, not {}", kind_of(Some(&m))))) };
    protocol_of(o, host)?;
    signature_of(o, trust)?;
    match o.get("needs") {
        None => {}
        Some(Value::Array(n)) if n.is_empty() => {}
        Some(_) => return Err(V1Error::new("not_supported", "needs: shared services are not supported in v1 (5.3); it must be absent or []")),
    }
    fields(o)?;
    let apps = o.get("apps").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    if let Some(i) = apps.iter().position(|a| get(a, "native").is_some()) {
        let key = o.get("publisher").and_then(|p| get(p, "key_id")).and_then(Value::as_str).unwrap_or_default();
        if !trust.publishers.get(key).is_some_and(|p| p.first_party) {
            return Err(invalid(&format!("apps[{i}].native"), "native screens are for first-party plugins only; ship the app as a TSX screen (entry)"));
        }
    }
    let sha256 = manifest_hash(&m).map_err(|e| invalid("manifest", e))?;
    Ok(Verified { manifest: m, sha256 })
}

// ── checks against the running plugin ────────────────────────────────────

/// The plugin's tools/list against its manifest: every tool but the
/// reserved `rox.*` follows the grammar and starts with `<short>_`
/// (MAN-10), every tool the manifest names exists, and a routine's writes
/// are tools that change data (5.9).
pub fn check_tools(m: &Value, tools: &[Value]) -> Res {
    let short = tpl(get(m, "short"));
    let mut names: Vec<&str> = Vec::new();
    for t in tools {
        let n = get(t, "name");
        if is_v1("reservedTool", n) {
            continue;
        }
        match n.and_then(Value::as_str) {
            Some(s) if super::grammar_ok("tool", s) && s.starts_with(&format!("{short}_")) => names.push(s),
            _ => return Err(invalid("tools/list", format!("tool {} must follow the tool grammar and start with {short}_", js::slice_utf16(&tpl(n), 80)))),
        }
    }
    let mut named: Vec<(Option<&Value>, String)> = Vec::new();
    named.extend(arr(get(m, "commands")).iter().map(|c| (get(c, "resolve"), "commands".to_owned())));
    named.extend(arr(get(m, "scopes")).iter().map(|s| (get(s, "resolve"), "scopes".to_owned())));
    named.extend(arr(get(m, "routines")).iter().map(|r| (get(r, "query").and_then(|q| get(q, "tool")), "routines".to_owned())));
    for a in arr(get(m, "apps")) {
        let at = format!("apps.{}.permissions", tpl(get(a, "id")));
        named.extend(arr(get(a, "permissions").and_then(|p| get(p, "tools"))).iter().map(|t| (Some(t), at.clone())));
    }
    for (t, at) in named {
        if !t.and_then(Value::as_str).is_some_and(|t| names.contains(&t)) {
            return Err(invalid(&at, format!("names {}, which the plugin's tools/list does not have", tpl(t))));
        }
    }
    for (i, r) in arr(get(m, "routines")).iter().enumerate() {
        for (k, w) in arr(get(r, "writes")).iter().enumerate() {
            let at = format!("routines[{i}].writes[{k}]");
            let Some(t) = tools.iter().find(|x| same(get(x, "name"), Some(w))) else {
                return Err(invalid(&at, format!("names {}, which the plugin's tools/list does not have", tpl(Some(w)))));
            };
            if get(t, "annotations").and_then(|a| get(a, "readOnlyHint")) == Some(&Value::Bool(true)) {
                return Err(invalid(&at, format!("{} is read-only (readOnlyHint): writes lists only tools that change data", tpl(Some(w)))));
            }
        }
    }
    Ok(())
}

// ── props (6.6) ──────────────────────────────────────────────────────────

fn bad(why: impl std::fmt::Display) -> V1Error {
    V1Error::new("invalid_args", format!("props: {why}"))
}

fn props_limits(v: &Value, depth: usize, path: &str) -> Res {
    if depth > L::PROPS_DEPTH {
        return Err(bad(format!("{path} nests deeper than {}", L::PROPS_DEPTH)));
    }
    match v {
        Value::Array(items) => {
            if items.len() > L::PROPS_ARRAY {
                return Err(bad(format!("{path} has over {} items", L::PROPS_ARRAY)));
            }
            for (i, x) in items.iter().enumerate() {
                props_limits(x, depth + 1, &format!("{path}[{i}]"))?;
            }
        }
        Value::Object(o) => {
            for (key, x) in js::ordered(o) {
                if !super::grammar_ok("propKey", key) {
                    return Err(bad(format!("key {} breaks the prop key grammar", js::slice_utf16(&js::quote(key), 40))));
                }
                props_limits(x, depth + 1, &format!("{path}.{key}"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn props_match(s: &Value, v: &Value, path: &str) -> Res {
    let num = |x: Option<&Value>| x.and_then(Value::as_f64);
    if let Some(ty) = get(s, "type") {
        let ok = match ty.as_str() {
            Some("integer") => v.as_f64().is_some_and(|f| f.is_finite() && f.fract() == 0.0),
            Some(t) => t == kind_of(Some(v)),
            None => false,
        };
        if !ok {
            return Err(bad(format!("{path} must be {}", tpl(Some(ty)))));
        }
    }
    if let Some(Value::Array(e)) = get(s, "enum") {
        let c = canonicalize(v);
        if !e.iter().any(|e| canonicalize(e) == c) {
            return Err(bad(format!("{path} is not one of the allowed values")));
        }
    }
    if let (Some(max), Value::String(t)) = (get(s, "maxLength"), v) {
        if num(Some(max)).is_some_and(|m| t.chars().count() as f64 > m) {
            return Err(bad(format!("{path} is over {} characters", tpl(Some(max)))));
        }
    }
    if let (Some(lo), Some(x)) = (get(s, "minimum"), v.as_f64()) {
        if num(Some(lo)).is_some_and(|lo| x < lo) {
            return Err(bad(format!("{path} is under {}", tpl(Some(lo)))));
        }
    }
    if let (Some(hi), Some(x)) = (get(s, "maximum"), v.as_f64()) {
        if num(Some(hi)).is_some_and(|hi| x > hi) {
            return Err(bad(format!("{path} is over {}", tpl(Some(hi)))));
        }
    }
    if let Value::Object(o) = v {
        for r in get(s, "required").and_then(Value::as_array).into_iter().flatten() {
            let r = js::string_of(r);
            if !o.contains_key(&r) {
                return Err(bad(format!("{path}.{r} is required")));
            }
        }
        for (key, sub) in get(s, "properties").and_then(Value::as_object).map(js::ordered).unwrap_or_default() {
            if let Some(x) = o.get(key) {
                props_match(sub, x, &format!("{path}.{key}"))?;
            }
        }
    }
    if let (Value::Array(items), Some(sub)) = (v, get(s, "items").filter(|i| js::truthy(i))) {
        for (i, x) in items.iter().enumerate() {
            props_match(sub, x, &format!("{path}[{i}]"))?;
        }
    }
    Ok(())
}

/// Props for an open of an app with props schema `s` (6.6): an object, at
/// most 8 KiB, depth 8, 100 items per array, prop-key grammar, and valid
/// against the schema subset. Refused as `invalid_args`.
pub fn validate_props(s: Option<&Value>, props: &Value) -> Res {
    if !props.is_object() {
        return Err(bad("must be a JSON object"));
    }
    let size = canonicalize(props).len();
    if size > L::PROPS_BYTES {
        return Err(bad(format!("is {size} bytes; at most {}", L::PROPS_BYTES)));
    }
    props_limits(props, 1, "props")?;
    match s {
        Some(s) if js::truthy(s) => props_match(s, props, "props"),
        _ => Ok(()),
    }
}

// ── install review (13.1 step 3) and update consent (13.2) ───────────────

/// `Math.round`: halves toward +∞.
fn round(x: f64) -> f64 {
    let r = x.round();
    if x < 0.0 && r - x == -0.5 {
        r + 1.0
    } else {
        r
    }
}

/// A byte count as the review shows it: `25 MB`, `512 KB`.
fn mb(n: Option<&Value>) -> String {
    let n = n.and_then(Value::as_f64).unwrap_or(f64::NAN);
    if n >= 1_048_576.0 {
        return format!("{} MB", js::number(round(n / 1_048_576.0 * 10.0) / 10.0));
    }
    let k = round(n / 1024.0);
    if k.is_nan() {
        "NaN KB".to_owned()
    } else {
        format!("{} KB", js::number(k.max(1.0)))
    }
}

/// `x ?? d` as a number.
fn or_default(v: Option<&Value>, d: u64) -> f64 {
    match v {
        None | Some(Value::Null) => d as f64,
        Some(v) => v.as_f64().unwrap_or(f64::NAN),
    }
}

/// `items.join(", ")`.
fn join(v: Option<&Value>) -> String {
    let items = v.and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    items.iter().map(|x| if x.is_null() { String::new() } else { js::string_of(x) }).collect::<Vec<_>>().join(", ")
}

fn at<'a>(p: Option<&'a Value>, k: &str) -> Option<&'a Value> {
    p.and_then(|p| get(p, k))
}

fn arr(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default()
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(js::truthy)
}

/// Whether `v?.length` is truthy for a list.
fn some(v: Option<&Value>) -> bool {
    v.and_then(Value::as_array).is_some_and(|a| !a.is_empty())
}

/// A scope's noun: its description's first word without trailing
/// punctuation ("Return:" → "Return"), else its name. (Node strips
/// `[^\p{L}\p{N}]`; `char::is_alphanumeric` also keeps the few combining
/// marks Unicode calls alphabetic.)
fn noun(s: Option<&Value>) -> Value {
    let pick = s.and_then(|s| get(s, "description")).filter(|d| js::truthy(d)).or_else(|| s.and_then(|s| get(s, "name")).filter(|n| js::truthy(n)));
    let text = pick.map(js::string_of).unwrap_or_default();
    let word = text.trim_matches(js::is_space).split(js::is_space).next().unwrap_or("").trim_end_matches(|c: char| !c.is_alphanumeric());
    if word.is_empty() {
        s.and_then(|s| get(s, "name")).cloned().unwrap_or(Value::Null)
    } else {
        Value::String(word.to_owned())
    }
}

fn permission_lines(m: &Value, p: Option<&Value>) -> Vec<String> {
    let Some(p) = p.filter(|p| js::truthy(p)) else { return Vec::new() };
    let mut lines = Vec::new();
    let title = |id: &Value| -> Value {
        let t = arr(get(m, "apps")).iter().find(|a| same(get(a, "id"), Some(id))).and_then(|a| get(a, "title"));
        t.filter(|t| js::truthy(t)).unwrap_or(id).clone()
    };
    let names = |vs: Vec<Value>| join(Some(&Value::Array(vs)));
    let tools = get(p, "tools");
    if some(tools) {
        let n = arr(tools).len();
        lines.push(format!("Can call {n} {} tool{}: {}", tpl(get(m, "title")), if n == 1 { "" } else { "s" }, join(tools)));
    }
    let open = get(p, "open");
    if some(open.and_then(|o| get(o, "apps"))) {
        lines.push(format!("Can open: {}", names(arr(open.and_then(|o| get(o, "apps"))).iter().map(title).collect())));
    }
    if some(open.and_then(|o| get(o, "external"))) {
        lines.push(format!("Can open in your browser (asks each time): {}", join(open.and_then(|o| get(o, "external")))));
    }
    if some(get(p, "attach")) {
        let scopes = arr(get(m, "scopes"));
        let nouns = arr(get(p, "attach")).iter().map(|s| noun(scopes.iter().find(|x| same(get(x, "name"), Some(s))))).collect();
        lines.push(format!("Can attach: {}", names(nouns)));
    }
    if let Some(f) = get(p, "pick_file").filter(|f| js::truthy(f)) {
        let ext = join(get(f, "extensions"));
        let ext = if ext.is_empty() { "any type".to_owned() } else { ext };
        let several = if truthy(get(f, "multiple")) { ", several at once" } else { "" };
        lines.push(format!("Can read files: {ext} up to {}{several}", mb(get(f, "max_bytes"))));
    }
    if let Some(st) = get(p, "storage").filter(|s| js::truthy(s)) {
        lines.push(format!("Storage: {}", mb(Some(&json!(or_default(get(st, "quota_bytes"), L::STORAGE_DEFAULT))))));
    }
    if truthy(get(p, "copy")) {
        lines.push("Copy to clipboard".to_owned());
    }
    if truthy(get(p, "preview")) {
        lines.push("Show print previews (static pages, no scripts)".to_owned());
    }
    lines
}

/// An object of the entries whose value is defined (`JSON.stringify`
/// drops `undefined`).
fn defined(entries: Vec<(&str, Option<Value>)>) -> Value {
    Value::Object(entries.into_iter().filter_map(|(k, v)| Some((k.to_owned(), v?))).collect())
}

/// What the install screen shows (13.1 step 3). `title_of(plugin)` names
/// another plugin a routine reads from.
pub fn review(m: &Value, title_of: &dyn Fn(Option<&Value>) -> String) -> Value {
    let mut apps = Vec::new();
    let mut app_lines = Vec::new();
    for a in arr(get(m, "apps")) {
        let placement = arr(get(a, "placement"));
        let inline = get(a, "inline");
        let size = |k| inline.and_then(|i| get(i, k)).cloned();
        let has_inline = placement.iter().any(|p| p == "inline");
        let lines = permission_lines(m, get(a, "permissions"));
        let shapes: Vec<String> = placement
            .iter()
            .map(|p| if p == "inline" { format!("inline {}×{}", tpl(size("width").as_ref()), tpl(size("height").as_ref())) } else { "full screen".to_owned() })
            .collect();
        app_lines.push(format!("{} ({})", tpl(get(a, "title")), shapes.join(", ")));
        app_lines.extend(lines.iter().map(|l| format!("  {l}")));
        apps.push(defined(vec![
            ("id", get(a, "id").cloned()),
            ("title", get(a, "title").cloned()),
            ("placement", get(a, "placement").cloned()),
            ("inline", Some(if has_inline { defined(vec![("width", size("width")), ("height", size("height"))]) } else { Value::Null })),
            ("lines", Some(json!(lines))),
        ]));
    }
    let mut routines = Vec::new();
    for r in arr(get(m, "routines")) {
        let rn = tpl(get(r, "name"));
        for x in arr(get(r, "reads")) {
            routines.push(format!("Routine '{rn}' reads via {}: {}", title_of(get(x, "plugin")), join(get(x, "tools"))));
        }
        if some(get(r, "writes")) {
            routines.push(format!("Routine '{rn}' may change {} data without asking: {}", tpl(get(m, "title")), join(get(r, "writes"))));
        }
        if truthy(get(r, "params")) {
            let props = get(r, "params").and_then(|p| get(p, "properties")).and_then(Value::as_object);
            let keys: Vec<&str> = props.map(js::ordered).unwrap_or_default().into_iter().map(|(k, _)| k.as_str()).collect();
            routines.push(format!("Routine '{rn}' runs per chat with its own settings: {}", keys.join(", ")));
        }
    }
    let publisher_name = get(m, "publisher").and_then(|p| get(p, "name"));
    let mut lines = vec![json!(format!("{} {} by {}", tpl(get(m, "title")), tpl(get(m, "version")), tpl(publisher_name)))];
    if let Some(d) = get(m, "description").filter(|d| js::truthy(d)) {
        lines.push(d.clone());
    }
    lines.extend(app_lines.into_iter().map(Value::String));
    lines.extend(routines.iter().cloned().map(Value::String));
    defined(vec![
        ("title", get(m, "title").cloned()),
        ("publisher", publisher_name.cloned()),
        ("version", get(m, "version").cloned()),
        ("description", Some(get(m, "description").filter(|d| js::truthy(d)).cloned().unwrap_or_else(|| json!("")))),
        ("apps", Some(Value::Array(apps))),
        ("routines", Some(json!(routines))),
        ("lines", Some(Value::Array(lines))),
    ])
}

/// `b.filter((x) => !a.includes(x))`.
fn added<'a>(a: Option<&Value>, b: Option<&'a Value>) -> Vec<&'a Value> {
    let a = arr(a);
    arr(b).iter().filter(|x| !a.iter().any(|y| same(Some(y), Some(x)))).collect()
}

/// The permissions `next` grants that `prev` did not (13.2: any added key
/// or entry needs consent; removals do not). Plain lines; empty = none.
pub fn added_permissions(prev: &Value, next: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let empty = json!({});
    for app in arr(get(next, "apps")) {
        let was = arr(get(prev, "apps")).iter().find(|a| same(get(a, "id"), get(app, "id"))).and_then(|a| get(a, "permissions")).filter(|p| js::truthy(p)).unwrap_or(&empty);
        let now = get(app, "permissions").filter(|p| js::truthy(p)).unwrap_or(&empty);
        let title = tpl(get(app, "title"));
        let mut push = |s: String| out.push(format!("{title}: {s}"));
        let (was, now) = (Some(was), Some(now));
        for t in added(at(was, "tools"), at(now, "tools")) {
            push(format!("can call {}", tpl(Some(t))));
        }
        for x in added(at(at(was, "open"), "apps"), at(at(now, "open"), "apps")) {
            push(format!("can open {}", tpl(Some(x))));
        }
        for x in added(at(at(was, "open"), "external"), at(at(now, "open"), "external")) {
            push(format!("can open {} in the browser", tpl(Some(x))));
        }
        for x in added(at(was, "attach"), at(now, "attach")) {
            push(format!("can attach {}", tpl(Some(x))));
        }
        if truthy(at(now, "pick_file")) {
            let w = at(was, "pick_file").filter(|w| js::truthy(w));
            let n = at(now, "pick_file");
            match w {
                None => push("can read files".to_owned()),
                Some(w) => {
                    for x in added(get(w, "extensions"), at(n, "extensions")) {
                        push(format!("can read {} files", tpl(Some(x))));
                    }
                    let bytes = |v: Option<&Value>| v.and_then(Value::as_f64).unwrap_or(f64::NAN);
                    if bytes(at(n, "max_bytes")) > bytes(get(w, "max_bytes")) {
                        push(format!("can read files up to {}", mb(at(n, "max_bytes"))));
                    }
                    if truthy(at(n, "multiple")) && !truthy(get(w, "multiple")) {
                        push("can read several files at once".to_owned());
                    }
                }
            }
        }
        let quota = |p: Option<&Value>| or_default(at(at(p, "storage"), "quota_bytes"), L::STORAGE_DEFAULT);
        if truthy(at(now, "storage")) && (!truthy(at(was, "storage")) || quota(now) > quota(was)) {
            push(format!("storage {}", mb(Some(&json!(quota(now))))));
        }
        if truthy(at(now, "copy")) && !truthy(at(was, "copy")) {
            push("copy to clipboard".to_owned());
        }
        let preview = |p: Option<&Value>| or_default(at(at(p, "preview"), "max_bytes"), L::PREVIEW_MAX_BYTES);
        if truthy(at(now, "preview")) && !truthy(at(was, "preview")) {
            push("show print previews".to_owned());
        } else if truthy(at(now, "preview")) && preview(now) > preview(was) {
            push(format!("print previews up to {}", mb(Some(&json!(preview(now))))));
        }
    }
    for r in arr(get(next, "routines")) {
        let was = arr(get(prev, "routines")).iter().find(|x| same(get(x, "id"), get(r, "id")));
        let rn = tpl(get(r, "name"));
        for x in arr(get(r, "reads")) {
            let had = was.and_then(|w| arr(get(w, "reads")).iter().find(|y| same(get(y, "plugin"), get(x, "plugin")))).and_then(|y| get(y, "tools"));
            for t in added(had, get(x, "tools")) {
                out.push(format!("Routine '{rn}' reads {}: {}", tpl(get(x, "plugin")), tpl(Some(t))));
            }
        }
        for t in added(was.and_then(|w| get(w, "writes")), get(r, "writes")) {
            out.push(format!("Routine '{rn}' may change {} data without asking: {}", tpl(get(next, "title")), tpl(Some(t))));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../../server/src/v1/fixture/manifest.json");

    fn signed(edit: impl FnOnce(&mut Value)) -> (String, Trust) {
        let key = rox_v1::keys::SessionKey::from_seed(&rox_v1::keys::b64(&[3; 32])).unwrap();
        let mut m: Value = serde_json::from_str(FIXTURE).unwrap();
        m["publisher"]["key_id"] = json!("test-key");
        edit(&mut m);
        let m = super::super::signing::sign_manifest(&m, &rox_v1::keys::b64(&[3; 32]), &[]).unwrap();
        let trust = Trust::from_value(&json!({
            "publishers": {"test-key": {"alg": "ed25519", "publisher": "app.buwiz", "public_key": key.public()}},
            "approved_plugins": [{"id": "app.buwiz.fixture", "publisher_key_id": "test-key"}]
        }));
        (js::stringify(&m), trust)
    }

    fn refused(edit: impl FnOnce(&mut Value)) -> V1Error {
        let (text, trust) = signed(edit);
        verify(&text, &trust, HOST_PROTOCOL).unwrap_err()
    }

    #[test]
    fn the_fixture_verifies_and_a_changed_byte_does_not() {
        let (text, trust) = signed(|_| {});
        let v = verify(&text, &trust, HOST_PROTOCOL).unwrap();
        assert_eq!(v.manifest["id"], "app.buwiz.fixture");
        assert_eq!(v.sha256.len(), 64);
        let e = verify(&text.replace("\"Fixture\"", "\"Fixturf\""), &trust, HOST_PROTOCOL).unwrap_err();
        assert_eq!(e.message, "signature: does not verify with the publisher key");
        assert_eq!(verify(&text, &Trust::default(), HOST_PROTOCOL).unwrap_err().message, "publisher.key_id: test-key is not a trusted publisher key");
    }

    #[test]
    fn refusals_name_the_field_and_carry_the_spec_code() {
        assert_eq!(refused(|m| m["protocol"] = json!("3.0.0")).code, "protocol_unsupported");
        assert_eq!(refused(|m| m["protocol"] = json!("2.2.0")).code, "protocol_unsupported");
        // a 2.1 host runs 2.0 and 2.1 plugins
        for p in ["2.0.0", "2.0.3", "2.1.0"] {
            let (text, trust) = signed(|m| m["protocol"] = json!(p));
            assert!(verify(&text, &trust, HOST_PROTOCOL).is_ok(), "{p}");
        }
        assert_eq!(HOST_PROTOCOL, "2.1.0");
        // a 1.x plugin speaks the rox names: told to update
        let old = refused(|m| m["protocol"] = json!("1.1.0"));
        assert_eq!((old.code.as_str(), old.message.as_str()), ("protocol_unsupported", "the plugin implements protocol 1.1.0: this plugin is built for an older protocol; update it"));
        assert_eq!(refused(|m| m["short"] = json!("buwiz")).message, "short: buwiz is reserved");
        assert!(RESERVED_ID_PREFIXES.iter().any(|p| "com.buwiz.bot.x".starts_with(p)) && !RESERVED_ID_PREFIXES.iter().any(|p| "app.buwiz.ebirforms".starts_with(p)));
        assert_eq!(refused(|m| m["needs"] = json!(["gmail"])).code, "not_supported");
        assert_eq!(refused(|m| m["commands"][0]["name"] = json!("status")).message, "commands[0].name: /status is a built-in command");
        assert_eq!(
            refused(|m| m["apps"][1]["permissions"]["camera"] = json!(true)).message,
            "apps[1].permissions.camera: is not a known key here (refused: unknown keys in access grants fail closed)"
        );
        assert_eq!(refused(|m| m["apps"][0]["permissions"]["open"]["external"] = json!(["https://example.com/"])).code, "manifest_invalid");
        assert_eq!(refused(|m| m["apps"][1]["inline"]["width"] = json!(961)).message, "apps[1].inline.width: must be 240 to 960");
        assert_eq!(refused(|m| m["apps"][1]["inline"]["wait_ready"] = json!("yes")).message, "apps[1].inline.wait_ready: must be a boolean");
    }

    #[test]
    fn an_app_is_tsx_or_native_and_native_needs_a_first_party_key() {
        fn native(m: &mut Value) {
            let a = &mut m["apps"][0];
            a["id"] = json!("ledger");
            for k in ["entry", "mime", "sdk", "fingerprint"] {
                a.as_object_mut().unwrap().remove(k);
            }
            a["native"] = json!({"screen": "fx.ledger", "abi": 1});
        }
        let (text, mut trust) = signed(native);
        assert_eq!(verify(&text, &trust, HOST_PROTOCOL).unwrap_err().message, "apps[0].native: native screens are for first-party plugins only; ship the app as a TSX screen (entry)");
        trust.publishers.get_mut("test-key").unwrap().first_party = true;
        assert!(verify(&text, &trust, HOST_PROTOCOL).is_ok());
        let first = |edit: fn(&mut Value)| {
            let (text, mut trust) = signed(edit);
            trust.publishers.get_mut("test-key").unwrap().first_party = true;
            verify(&text, &trust, HOST_PROTOCOL).unwrap_err().message
        };
        assert_eq!(first(|m| m["apps"][0]["native"] = json!({"screen": "fx.board", "abi": 1})), "apps[0]: is a TSX screen (entry) or a native screen (native), not both");
        assert_eq!(first(|m| drop(m["apps"][0].as_object_mut().unwrap().remove("entry"))), "apps[0]: needs entry (a TSX screen) or native (a native screen)");
        assert_eq!(first(|m| {
            native(m);
            m["apps"][0]["native"]["abi"] = json!(2);
        }), "apps[0].native.abi: must be 1");
    }

    #[test]
    fn inline_wait_ready_is_accepted() {
        for v in [true, false] {
            let (text, trust) = signed(|m| m["apps"][1]["inline"]["wait_ready"] = json!(v));
            assert!(verify(&text, &trust, HOST_PROTOCOL).is_ok());
        }
    }

    #[test]
    fn semver_orders_by_precedence() {
        assert_eq!(compare_semver("1.0.0-alpha", "1.0.0"), Some(-1));
        assert_eq!(compare_semver("1.10.0", "1.9.9"), Some(1));
        assert_eq!(compare_semver("1.0.0-alpha.1", "1.0.0-alpha.beta"), Some(-1));
        assert_eq!(compare_semver("1.0.0+b1", "1.0.0+b2"), Some(0));
        assert_eq!(compare_semver("1.0", "1.0.0"), None);
        assert!(semver("1.0.0-01").is_none() && semver("01.0.0").is_none() && semver("1.0.0-").is_none());
    }

    #[test]
    fn review_sizes_read_as_node_prints_them() {
        assert_eq!(mb(Some(&json!(26_214_400))), "25 MB");
        assert_eq!(mb(Some(&json!(3_145_728))), "3 MB");
        assert_eq!(mb(Some(&json!(1_500_000))), "1.4 MB");
        assert_eq!(mb(Some(&json!(100))), "1 KB");
        assert_eq!(mb(None), "NaN KB");
    }
}
