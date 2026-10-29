//! Loading a v1 app (spec 6): its source from the broker, the fingerprint
//! checked again here, the import table checked before anything is added,
//! props validated, then the module the engine runs: compiled, the bridge
//! shim in front, the default export mounted.
//!
//! The import check runs twice. On the source, it refuses any specifier
//! outside the table (6.3) and any name of `nsh` outside the SDK-1 export
//! set, before the shim adds its own `nsh/host` import. On the compiled
//! module (no JSX or types left, so strings and code are told apart
//! reliably), it refuses `import()` and anything the compiler kept that
//! the table does not list. `nsh/host` is out of the table (it is `host`
//! of `nsh` in nsh 0.3), so an app never reaches the channel itself.

use serde_json::{Map, Value};

use super::{RoxError, V1Key, host_api, keys, perms::Permissions};

/// The source size limit (6.1).
pub const MAX_SOURCE: usize = 512 * 1024;

/// What `nsh` exports to an SDK-1 app (6.3): values and types.
pub const SDK1_NSH: &[&str] = &[
    "signal", "computed", "effect", "untracked", "resource", "cleanup", "mount", "Text", "View", "Scroll", "Input",
    "Overlay", "JSX", "Signal", "Accessor", "YNode",
];

/// What `rox` exports: the bridge and its error class.
const ROX_EXPORTS: &[&str] = &["rox", "RoxError"];

/// The shim prepended to every v1 app.
pub const SHIM: &str = include_str!("shim.js");

/// The user-facing texts of spec 6.7.
pub mod say {
    pub fn compile(title: &str, first_line: &str, line: Option<usize>) -> String {
        match line {
            Some(n) => format!("Could not open {title}: {first_line} (line {n})."),
            None if first_line.ends_with(['.', '?', '!']) => format!("Could not open {title}: {first_line}"),
            None => format!("Could not open {title}: {first_line}."),
        }
    }

    pub const FINGERPRINT: &str = "This app's code does not match its signed manifest. Update the plugin.";

    pub fn unsupported(title: &str, specifier: &str) -> String {
        format!("{title} uses {specifier}, which this Rox version does not support.")
    }

    pub fn too_long(title: &str) -> String {
        format!("{title} stopped: it took too long.")
    }

    pub fn out_of_memory(title: &str) -> String {
        format!("{title} stopped: it ran out of memory.")
    }

    pub fn runtime(title: &str, message: &str) -> String {
        let message: String = message.trim().trim_end_matches('.').chars().take(200).collect();
        format!("{title} stopped: {message}.")
    }

    pub fn unavailable(plugin_title: &str) -> String {
        format!("{plugin_title} is not running. Retry")
    }
}

/// What a running app's error shows (6.7), from the engine's report: the
/// kind (`nsh_engine::AppErrorKind`) and its text. `None` for errors the
/// app keeps running after but that are not uncaught (assets).
pub fn runtime_message(title: &str, kind: nsh_engine::AppErrorKind, text: &str) -> Option<String> {
    use nsh_engine::AppErrorKind as K;
    let lower = text.to_ascii_lowercase();
    let first = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("it failed");
    match kind {
        K::Load | K::Stopped | K::Uncaught if lower.contains("out of memory") || lower.contains("memory limit") => {
            Some(say::out_of_memory(title))
        }
        K::Load | K::Stopped | K::Uncaught if lower.contains("interrupted") || lower.contains("budget") || lower.contains("took too long") => {
            Some(say::too_long(title))
        }
        K::Load | K::Stopped | K::Uncaught => Some(say::runtime(title, first)),
        _ => None,
    }
}

/// `sha256:<hex>` of `source`'s bytes, as returned (6.2).
pub fn fingerprint(source: &str) -> String {
    format!("sha256:{}", keys::sha256_hex(source.as_bytes()))
}

// ---------------------------------------------------------------------------
// A small JS tokenizer: enough to tell imports from strings and comments.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Ident,
    Str,
    Punct,
    Other,
}

#[derive(Debug, Clone)]
struct Tok {
    kind: Kind,
    text: String,
    start: usize,
    end: usize,
}

/// Words after which a `/` starts a regular expression.
const BEFORE_REGEX: &[&str] = &[
    "return", "typeof", "case", "do", "else", "in", "of", "new", "delete", "void", "throw", "instanceof", "yield",
    "await",
];

/// The tokens of `src`; template literals' `${…}` parts are tokenized too.
fn tokens(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let mut out: Vec<Tok> = Vec::new();
    // modes: true = a template's text, false = code (with its brace depth)
    let mut stack: Vec<(bool, usize)> = vec![(false, 0)];
    let mut i = 0;
    let push = |out: &mut Vec<Tok>, kind, start: usize, end: usize, text: &str| {
        out.push(Tok { kind, text: text.to_owned(), start, end })
    };
    while i < b.len() {
        let (in_template, depth) = *stack.last().expect("a mode");
        if in_template {
            match b[i] {
                b'\\' => i += 2,
                b'`' => {
                    stack.pop();
                    i += 1;
                }
                b'$' if b.get(i + 1) == Some(&b'{') => {
                    stack.push((false, 0));
                    i += 2;
                }
                _ => i += 1,
            }
            continue;
        }
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            i = src[i..].find('\n').map_or(b.len(), |n| i + n);
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i = src[i + 2..].find("*/").map_or(b.len(), |n| i + n + 4);
        } else if c == b'"' || c == b'\'' {
            let start = i;
            i += 1;
            while i < b.len() && b[i] != c && b[i] != b'\n' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            let end = (i + 1).min(b.len());
            push(&mut out, Kind::Str, start, end, src.get(start + 1..i).unwrap_or_default());
            i = end;
        } else if c == b'`' {
            push(&mut out, Kind::Other, i, i + 1, "`");
            stack.push((true, 0));
            i += 1;
        } else if c == b'/' && regex_allowed(out.last()) {
            let start = i;
            i += 1;
            let mut class = false;
            while i < b.len() && b[i] != b'\n' && (b[i] != b'/' || class) {
                match b[i] {
                    b'\\' => i += 1,
                    b'[' => class = true,
                    b']' => class = false,
                    _ => {}
                }
                i += 1;
            }
            i += 1;
            while i < b.len() && b[i].is_ascii_alphabetic() {
                i += 1;
            }
            push(&mut out, Kind::Other, start, i.min(b.len()), "/re/");
        } else if c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c >= 0x80 {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$' || b[i] >= 0x80) {
                i += 1;
            }
            push(&mut out, Kind::Ident, start, i, &src[start..i]);
        } else if c.is_ascii_digit() {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'_') {
                i += 1;
            }
            push(&mut out, Kind::Other, start, i, "0");
        } else {
            if c == b'{' {
                stack.last_mut().expect("a mode").1 += 1;
            } else if c == b'}' {
                if depth == 0 && stack.len() > 1 {
                    // the end of a template's `${…}`
                    stack.pop();
                    i += 1;
                    continue;
                }
                stack.last_mut().expect("a mode").1 = depth.saturating_sub(1);
            }
            push(&mut out, Kind::Punct, i, i + 1, &src[i..i + 1]);
            i += 1;
        }
    }
    out
}

/// Whether a `/` after `prev` starts a regular expression.
fn regex_allowed(prev: Option<&Tok>) -> bool {
    match prev {
        None => true,
        Some(t) if t.kind == Kind::Punct => !matches!(t.text.as_str(), ")" | "]" | "}"),
        Some(t) if t.kind == Kind::Ident => BEFORE_REGEX.contains(&t.text.as_str()),
        Some(_) => false,
    }
}

/// One `import … from "x"` / `export … from "x"` / `import "x"`.
#[derive(Debug, Clone)]
struct Import {
    spec: String,
    /// The tokens between the keyword and `from` (empty for `import "x"`).
    clause: Vec<Tok>,
    /// Byte range of the whole statement, its `;` included.
    start: usize,
    end: usize,
    export: bool,
}

/// The module's static imports and re-exports; `Err("import()")` for a
/// dynamic import.
fn imports(src: &str, toks: &[Tok]) -> Result<Vec<Import>, String> {
    let mut out = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        let next = toks.get(i + 1);
        if t.kind != Kind::Ident || (t.text != "import" && t.text != "export") {
            continue;
        }
        // `x.import`: a property, not the keyword
        if i > 0 && toks[i - 1].text == "." {
            continue;
        }
        if t.text == "import" && next.is_some_and(|n| n.text == "(") {
            return Err("import()".to_owned());
        }
        if t.text == "import" && next.is_some_and(|n| n.text == ".") {
            continue;
        }
        if t.text == "import" && next.is_some_and(|n| n.kind == Kind::Str) {
            let n = next.expect("checked");
            out.push(Import { spec: n.text.clone(), clause: Vec::new(), start: t.start, end: semi(src, toks, i + 1), export: false });
            continue;
        }
        // only `export {…} from` and `export * from` re-export
        if t.text == "export" && !next.is_some_and(|n| ["{", "*", "type"].contains(&n.text.as_str())) {
            continue;
        }
        // the clause runs to `from "…"`; a statement's end first means none
        let mut j = i + 1;
        let mut clause = Vec::new();
        let found = loop {
            let Some(x) = toks.get(j) else { break None };
            let after = toks.get(j + 1);
            if x.text == "from" && after.is_some_and(|s| s.kind == Kind::Str) {
                break Some(j + 1);
            }
            let keyword = x.kind == Kind::Ident && (x.text == "import" || x.text == "export");
            if x.text == ";" || keyword || (x.text == "}" && !after.is_some_and(|n| n.text == "from")) {
                break None;
            }
            clause.push(x.clone());
            j += 1;
        };
        if let Some(s) = found {
            out.push(Import { spec: toks[s].text.clone(), clause, start: t.start, end: semi(src, toks, s), export: t.text == "export" });
        }
    }
    Ok(out)
}

/// The end of the statement whose last token is `toks[i]`: after its `;`.
fn semi(src: &str, toks: &[Tok], i: usize) -> usize {
    match toks.get(i + 1) {
        Some(t) if t.text == ";" => t.end,
        _ => toks[i].end.min(src.len()),
    }
}

/// The names a clause imports: `(imported, local, type_only)`; `None` for
/// a namespace (`* as x`) or `export *`.
fn names(clause: &[Tok]) -> Option<Vec<(String, String, bool)>> {
    let mut out = Vec::new();
    let mut all_types = false;
    let mut i = 0;
    if clause.first().is_some_and(|t| t.text == "type") && clause.len() > 1 {
        all_types = true;
        i = 1;
    }
    let mut braced = false;
    while i < clause.len() {
        let t = &clause[i];
        match t.text.as_str() {
            "*" => return None,
            "{" => braced = true,
            "}" => braced = false,
            "," => {}
            _ if t.kind == Kind::Ident => {
                let ty = braced && t.text == "type" && clause.get(i + 1).is_some_and(|n| n.kind == Kind::Ident && n.text != "as");
                if ty {
                    i += 1;
                }
                let name = clause[i].text.clone();
                let mut local = name.clone();
                if clause.get(i + 1).is_some_and(|n| n.text == "as") {
                    local = clause.get(i + 2).map(|n| n.text.clone()).unwrap_or_default();
                    i += 2;
                }
                // a default import binds `default`
                let imported = if braced { name } else { "default".to_owned() };
                out.push((imported, local, all_types || ty));
            }
            _ => {}
        }
        i += 1;
    }
    Some(out)
}

/// Checks the app's own source against the import table (6.3): the
/// offending specifier (or `nsh`'s name) when it imports outside it.
pub fn check_source_imports(source: &str) -> Result<(), String> {
    let toks = tokens(source);
    // the shim's bindings share the app's module scope: the app may not
    // name them (its `nsh` host binding above all)
    if let Some(t) = toks.iter().find(|t| t.kind == Kind::Ident && t.text.starts_with("__rox")) {
        return Err(t.text.clone());
    }
    for imp in imports(source, &toks)? {
        check_import(&imp, false)?;
    }
    Ok(())
}

fn check_import(imp: &Import, compiled: bool) -> Result<(), String> {
    let spec = imp.spec.as_str();
    match spec {
        "nsh/base" => Ok(()),
        // the compiler's own runtime import (JSX), never the app's
        "nsh:rt" if compiled => Ok(()),
        "nsh" => {
            let Some(names) = names(&imp.clause) else { return Err("nsh's namespace".to_owned()) };
            for (imported, _, type_only) in names {
                if !type_only && !SDK1_NSH.contains(&imported.as_str()) {
                    return Err(format!("{imported} from nsh"));
                }
            }
            Ok(())
        }
        "rox" if !imp.export => {
            let Some(names) = names(&imp.clause) else { return Err("rox's namespace".to_owned()) };
            for (imported, _, type_only) in names {
                if !type_only && imported != "default" && !ROX_EXPORTS.contains(&imported.as_str()) {
                    return Err(format!("{imported} from rox"));
                }
            }
            Ok(())
        }
        _ => Err(spec.to_owned()),
    }
}

/// The compiled module with its `rox` imports bound to the globals the shim
/// sets, after checking its imports again: the offending specifier, or
/// `import()`.
fn bind_rox_imports(js: &str) -> Result<String, String> {
    let toks = tokens(js);
    let mut out = String::with_capacity(js.len());
    let mut at = 0;
    for imp in imports(js, &toks)? {
        check_import(&imp, true)?;
        if imp.spec != "rox" {
            continue;
        }
        let mut bind = String::new();
        for (imported, local, type_only) in names(&imp.clause).unwrap_or_default() {
            if type_only {
                continue;
            }
            let global = if imported == "RoxError" { "RoxError" } else { "rox" };
            bind.push_str(&format!("const {local} = globalThis.{global};"));
        }
        out.push_str(&js[at..imp.start]);
        out.push_str(&bind);
        at = imp.end;
    }
    out.push_str(&js[at..]);
    Ok(out)
}

/// What the module mounts (6.3): its default export, or (v0 apps only) a
/// binding named `R`, with a deprecation. The module, changed to name an
/// anonymous default, and the binding.
fn mount_target(js: &str) -> Result<(String, String, bool), String> {
    let toks = tokens(js);
    for (i, t) in toks.iter().enumerate() {
        if t.text != "export" || t.kind != Kind::Ident {
            continue;
        }
        let at = |k: usize| toks.get(i + k).map(|t| t.text.as_str()).unwrap_or_default();
        if at(1) == "default" {
            let named = |k: usize| toks.get(i + k).filter(|t| t.kind == Kind::Ident).map(|t| t.text.clone());
            let name = match (at(2), at(3)) {
                ("function", "*") => named(4),
                ("function" | "class", _) => named(3),
                ("async", "function") => named(4).filter(|_| at(4) != "*").or_else(|| named(5).filter(|_| at(4) == "*")),
                _ => None,
            };
            if let Some(name) = name.filter(|n| n != "extends") {
                return Ok((js.to_owned(), name, false));
            }
            // an anonymous default: give it a name
            let (start, end) = (t.start, toks[i + 1].end);
            let js = format!("{}const __roxDefault ={}", &js[..start], &js[end..]);
            return Ok((js, "__roxDefault".to_owned(), false));
        }
        if at(1) == "{" {
            // `export { X as default }`
            let mut j = i + 2;
            while let Some(x) = toks.get(j).filter(|x| x.text != "}") {
                if x.text == "as" && toks.get(j + 1).is_some_and(|d| d.text == "default") {
                    return Ok((js.to_owned(), toks[j - 1].text.clone(), false));
                }
                j += 1;
            }
        }
    }
    let binds_r = toks.windows(2).any(|w| {
        ["function", "const", "let", "var", "class"].contains(&w[0].text.as_str()) && w[1].text == "R"
    });
    if binds_r {
        return Ok((js.to_owned(), "R".to_owned(), true));
    }
    Err("it has no default export".to_owned())
}

/// The first line of a compile error and its line number.
fn compile_error(error: &str) -> (String, Option<usize>) {
    let first = error
        .lines()
        .map(|l| l.trim().trim_start_matches(['x', '×']).trim())
        .find(|l| !l.is_empty())
        .unwrap_or("it failed to compile")
        .to_owned();
    // oxc's frame names the place as `,-[line:column]` (or `file:line:column`)
    let line = [",-[", ".tsx:"]
        .iter()
        .find_map(|mark| error.find(mark).map(|at| &error[at + mark.len()..]))
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|n| n.parse().ok());
    (first, line)
}

/// `source` (the app's, checked) as the module the engine runs: compiled,
/// its `rox` imports bound, the shim in front, its default export mounted.
/// `title` names it in errors.
pub fn build(app_id: &str, title: &str, source: &str) -> Result<String, RoxError> {
    if let Err(spec) = check_source_imports(source) {
        return Err(RoxError::new("unsupported_import", say::unsupported(title, &spec)));
    }
    let homed = crate::mini::current_homes(source);
    let sets: Vec<nsh_core::element::description::SetDescription> =
        vec![nsh_base::registry().into(), crate::registry().into()];
    let js = nsh_bundler::compile_module(&format!("{app_id}.tsx"), &homed, &sets).map_err(|e| {
        let (first, line) = compile_error(&e);
        RoxError::new("compile_error", say::compile(title, &first, line))
    })?;
    let js = bind_rox_imports(&js).map_err(|spec| RoxError::new("unsupported_import", say::unsupported(title, &spec)))?;
    let (js, target, deprecated) =
        mount_target(&js).map_err(|e| RoxError::new("compile_error", say::compile(title, &e, None)))?;
    if deprecated {
        log::warn!("mini v1: {app_id} has no default export; mounting its R binding (deprecated, v0 only)");
    }
    Ok(format!("{SHIM}\n{js}\nimport {{ mount as __roxMount }} from \"nsh\";\n__roxMount({target});\n"))
}

/// A v1 plugin as the broker lists it (`GET /v1/host/plugins`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PluginEntry {
    pub id: String,
    pub short: String,
    pub title: String,
    pub version: String,
    pub protocol: String,
    pub generation: Option<u64>,
    pub enabled: bool,
    /// An approved plugin of the first-party publisher (9.8).
    pub first_party: bool,
    /// Its manifest's `scopes`, `apps` and `commands`.
    pub scopes: Vec<Value>,
    pub apps: Vec<Value>,
    pub commands: Vec<Value>,
}

impl PluginEntry {
    pub fn from_json(v: &Value) -> Option<PluginEntry> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        let list = |k: &str| v.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
        Some(PluginEntry {
            id: v.get("id")?.as_str()?.to_owned(),
            short: s("short"),
            title: s("title"),
            version: s("version"),
            protocol: s("protocol"),
            generation: v.get("generation").and_then(Value::as_u64),
            enabled: v.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            first_party: v.get("first_party").and_then(Value::as_bool).unwrap_or(false),
            scopes: list("scopes"),
            apps: list("apps"),
            commands: list("commands"),
        })
    }

    /// Its manifest entry of app `id`.
    pub fn app(&self, id: &str) -> Option<&Value> {
        self.apps.iter().find(|a| a.get("id").and_then(Value::as_str) == Some(id))
    }

    /// Whether scope `name` declares `attach: true`.
    pub fn scope_attaches(&self, name: &str) -> bool {
        self.scopes
            .iter()
            .any(|s| s.get("name").and_then(Value::as_str) == Some(name) && s.get("attach") == Some(&Value::Bool(true)))
    }
}

/// An app ready to open: its compiled module and what its opens need.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub key: V1Key,
    pub plugin: PluginEntry,
    pub title: String,
    pub fingerprint: String,
    /// `apps[].permissions` as the manifest has them (their digest is
    /// what the certificate covers).
    pub permissions_json: Option<Value>,
    pub permissions: Permissions,
    pub js: String,
}

impl Prepared {
    /// `rox.app` (7.2).
    pub fn app_info(&self) -> Value {
        serde_json::json!({
            "plugin": self.plugin.id, "app": self.key.app, "version": self.plugin.version,
            "fingerprint": self.fingerprint, "protocol": self.plugin.protocol, "sdk": "1",
        })
    }
}

/// Everything before an open's session: the plugin and app as listed,
/// their permissions, the props, the source, its fingerprint, the import
/// table, the module. Errors carry the 6.7 text as their message.
pub fn prepare(broker: &dyn host_api::Broker, key: &str) -> Result<Prepared, RoxError> {
    prepare_in(broker, key, None)
}

/// [`prepare`] with the compiled-module cache under `cache_root` (the
/// plugins dir; spec 14.6): a hit skips the source fetch and the compile.
pub fn prepare_in(broker: &dyn host_api::Broker, key: &str, cache_root: Option<&std::path::Path>) -> Result<Prepared, RoxError> {
    let t0 = std::time::Instant::now();
    let key = V1Key::parse(key)?;
    let plugin = host_api::plugins(broker)?
        .iter()
        .filter_map(PluginEntry::from_json)
        .find(|p| p.id == key.plugin)
        .ok_or_else(|| RoxError::new("not_found", format!("{} is not installed.", key.plugin)))?;
    if !plugin.enabled {
        return Err(RoxError::new("plugin_unavailable", format!("{} is disabled.", plugin.title)));
    }
    let listed = t0.elapsed();
    let r = prepare_cached(broker, key, plugin, cache_root);
    if let Ok(p) = &r {
        log::info!("mini v1 timing: {}/{} plugin list {} ms", p.plugin.id, p.key.app, listed.as_millis());
    }
    r
}

/// [`prepare_with`] through the cache under `cache_root`, if any.
pub fn prepare_cached(
    broker: &dyn host_api::Broker,
    mut key: V1Key,
    plugin: PluginEntry,
    cache_root: Option<&std::path::Path>,
) -> Result<Prepared, RoxError> {
    let Some(root) = cache_root else { return prepare_with(broker, key, plugin) };
    let t0 = std::time::Instant::now();
    let app = plugin.app(&key.app).cloned();
    let declared = app.as_ref().and_then(|a| a.get("fingerprint")).and_then(Value::as_str).unwrap_or_default().to_owned();
    if let (Some(app), Some(js)) = (&app, super::cache::get(root, &plugin.id, &key.app, &declared, &super::cache::build_id())) {
        // the checks that do not need the source still run on a hit
        let title = app.get("title").and_then(Value::as_str).unwrap_or(&key.app).to_owned();
        let permissions_json = app.get("permissions").cloned();
        let permissions = Permissions::parse(permissions_json.as_ref())
            .map_err(|e| RoxError::new("manifest_invalid", format!("{title}'s permissions are invalid: {}", e.message)))?;
        ticket_prop(&mut key, app.get("props"));
        super::props::validate(&key.props, app.get("props"))?;
        log::info!("mini v1 timing: {}/{} cache hit {} ms (no fetch, no compile)", plugin.id, key.app, t0.elapsed().as_millis());
        return Ok(Prepared { key, plugin, title, fingerprint: declared, permissions_json, permissions, js });
    }
    let p = prepare_with(broker, key, plugin)?;
    super::cache::put(root, &p.plugin.id, &p.key.app, &p.fingerprint, &super::cache::build_id(), &p.js);
    Ok(p)
}

/// Prewarm (spec 14.6): fetches, checks and compiles `app` of `plugin` into
/// the cache under `root` unless it is there. No props, no session, no
/// engine. Whether it compiled (false: a hit).
pub fn warm(broker: &dyn host_api::Broker, plugin: &PluginEntry, app: &str, root: &std::path::Path) -> Result<bool, RoxError> {
    let entry = plugin.app(app).ok_or_else(|| RoxError::new("not_found", format!("{} has no app {app}", plugin.id)))?;
    let declared = entry.get("fingerprint").and_then(Value::as_str).unwrap_or_default();
    let rox = super::cache::build_id();
    if super::cache::get(root, &plugin.id, app, declared, &rox).is_some() {
        return Ok(false);
    }
    let title = entry.get("title").and_then(Value::as_str).unwrap_or(app);
    let (source, served) = host_api::app_source(broker, &plugin.id, app)?;
    if fingerprint(&source) != declared || served != declared || source.len() > MAX_SOURCE {
        return Err(RoxError::new("fingerprint_mismatch", say::FINGERPRINT));
    }
    let js = build(app, title, &source)?;
    super::cache::put(root, &plugin.id, app, declared, &rox, &js);
    Ok(true)
}

/// [`prepare`] for a plugin already listed.
pub fn prepare_with(broker: &dyn host_api::Broker, mut key: V1Key, plugin: PluginEntry) -> Result<Prepared, RoxError> {
    let app = plugin.app(&key.app).cloned().ok_or_else(|| {
        RoxError::new("not_found", format!("{} has no app {}.", plugin.title, key.app))
    })?;
    let title = app.get("title").and_then(Value::as_str).unwrap_or(&key.app).to_owned();
    if app.get("sdk").and_then(Value::as_str).is_some_and(|sdk| sdk != "1") {
        return Err(RoxError::new("not_supported", format!("{title} needs a newer Rox.")));
    }
    let permissions_json = app.get("permissions").cloned();
    let permissions = Permissions::parse(permissions_json.as_ref())
        .map_err(|e| RoxError::new("manifest_invalid", format!("{title}'s permissions are invalid: {}", e.message)))?;
    ticket_prop(&mut key, app.get("props"));
    super::props::validate(&key.props, app.get("props"))?;
    let t_fetch = std::time::Instant::now();
    let (source, served) = host_api::app_source(broker, &plugin.id, &key.app).map_err(|e| match e.code.as_str() {
        "fingerprint_mismatch" => RoxError::new("fingerprint_mismatch", say::FINGERPRINT),
        "plugin_unavailable" => RoxError::new("plugin_unavailable", say::unavailable(&plugin.title)),
        _ => e,
    })?;
    let declared = app.get("fingerprint").and_then(Value::as_str).unwrap_or_default();
    let actual = fingerprint(&source);
    if source.len() > MAX_SOURCE {
        return Err(RoxError::new("invalid_args", say::compile(&title, "its code is over 512 KiB", None)));
    }
    if actual != declared || served != declared {
        log::warn!("mini v1: {}/{}: fingerprint {actual} is not the manifest's {declared}", plugin.id, key.app);
        return Err(RoxError::new("fingerprint_mismatch", say::FINGERPRINT));
    }
    let t_compile = std::time::Instant::now();
    let js = build(&key.app, &title, &source)?;
    log::info!(
        "mini v1 timing: {}/{} source fetch {} ms, checks+compile {} ms",
        plugin.id,
        key.app,
        (t_compile - t_fetch).as_millis(),
        t_compile.elapsed().as_millis()
    );
    Ok(Prepared { key, plugin, title, fingerprint: actual, permissions_json, permissions, js })
}

/// The open's ticket (a link's or pin's `?ticket=`) as `rox.props.ticket`
/// when the app's props schema declares `ticket` and the props carry none
/// (spec 6.6): a link or pin that names only a ticket opens its subject.
fn ticket_prop(key: &mut V1Key, schema: Option<&Value>) {
    let declared = schema.and_then(|s| s.get("properties")).and_then(|p| p.get("ticket")).is_some();
    if let (true, Some(t)) = (declared, &key.ticket) {
        key.props.entry("ticket").or_insert_with(|| Value::String(t.clone()));
    }
}

/// `props` as the open's `rox.props`.
pub fn props_value(props: &Map<String, Value>) -> Value {
    Value::Object(props.clone())
}

#[cfg(test)]
mod tests {
    #[test]
    fn app_05_a_ticket_only_open_gives_the_app_its_ticket_when_its_props_declare_one() {
        let with = serde_json::json!({ "type": "object", "properties": { "ticket": { "type": "string" } } });
        let mut key = V1Key::parse("v1:dev.goldcoders.bir/tax-profile?ticket=tkt_0123456789ABCDEFGHJKMNPQRS").unwrap();
        super::ticket_prop(&mut key, Some(&with));
        assert_eq!(key.props.get("ticket").and_then(Value::as_str), Some("tkt_0123456789ABCDEFGHJKMNPQRS"));
        // props that name a ticket keep theirs; an app without one gets none
        let mut own = V1Key::parse("v1:dev.goldcoders.bir/tax-profile?ticket=tkt_0123456789ABCDEFGHJKMNPQRS&props=%7B%22ticket%22%3A%22tkt_ZZZZZZZZZZZZZZZZZZZZZZZZZZ%22%7D").unwrap();
        super::ticket_prop(&mut own, Some(&with));
        assert_eq!(own.props.get("ticket").and_then(Value::as_str), Some("tkt_ZZZZZZZZZZZZZZZZZZZZZZZZZZ"));
        let mut none = V1Key::parse("v1:dev.goldcoders.fixture/board?ticket=tkt_0123456789ABCDEFGHJKMNPQRS").unwrap();
        super::ticket_prop(&mut none, None);
        assert!(none.props.is_empty());
    }

    use super::*;
    use crate::mini_v1::host_api::Broker as _;

    #[test]
    fn app_03_imports_outside_the_table_are_unsupported() {
        let ok = [
            "import { signal, Text, View } from \"nsh\";",
            "import { Button, Input } from 'nsh/base';",
            "import { rox } from \"rox\";",
            "import type { Foo, Bar } from \"nsh\";",
            "import { type ViewStyle, signal as s } from \"nsh\";",
            "import rox, { RoxError } from \"rox\";",
            "const s = \"import x from 'nsh/device'\"; // import { host } from \"nsh\"",
            "/* import fs from \"fs\" */ const t = `import(${1})`;",
        ];
        for src in ok {
            assert_eq!(check_source_imports(src), Ok(()), "{src}");
        }
        let refused = [
            ("import { windowSize } from \"nsh/device\";", "nsh/device"),
            ("import { Clipboard } from \"nsh/clipboard\";", "nsh/clipboard"),
            ("import { host } from \"nsh\";", "host from nsh"),
            ("import { host as h } from \"nsh/host\";", "nsh/host"),
            ("import * as N from \"nsh\";", "nsh's namespace"),
            ("import N from \"nsh\";", "default from nsh"),
            ("import \"nsh/desktop\";", "nsh/desktop"),
            ("import _ from \"lodash\";", "lodash"),
            ("import { x } from \"./local\";", "./local"),
            ("export { host } from \"nsh\";", "host from nsh"),
            ("export * from \"nsh/device\";", "nsh/device"),
            ("const m = await import(\"nsh\");", "import()"),
            ("__roxHost.call(\"rox.open\", {});", "__roxHost"),
            ("const h = __roxBridge;", "__roxBridge"),
            ("const t = `${import(\"nsh\")}`;", "import()"),
            ("import { x } from \"nsh:rt\";", "nsh:rt"),
            ("import { fetchy } from \"rox\";", "fetchy from rox"),
        ];
        for (src, spec) in refused {
            assert_eq!(check_source_imports(src), Err(spec.to_owned()), "{src}");
        }
        let e = build("forms", "BIR Forms", "import { windowSize } from \"nsh/device\";\nexport default function F() { return null; }").unwrap_err();
        assert_eq!(e.code, "unsupported_import");
        assert_eq!(e.message, "BIR Forms uses nsh/device, which this Rox version does not support.");
    }

    #[test]
    fn app_02_the_default_export_mounts_and_r_is_a_deprecated_fallback() {
        let named = build("card", "Card", "import { Text } from \"nsh\";\nexport default function Card() { return <Text>hi</Text>; }").unwrap();
        assert!(named.ends_with("__roxMount(Card);\n"), "{named}");
        assert!(named.starts_with(SHIM), "the shim comes first");
        let anon = build("card", "Card", "import { Text } from \"nsh\";\nexport default () => <Text>hi</Text>;").unwrap();
        assert!(anon.contains("const __roxDefault ="), "{anon}");
        assert!(anon.ends_with("__roxMount(__roxDefault);\n"), "{anon}");
        let listed = build("card", "Card", "import { Text } from \"nsh\";\nfunction Card() { return <Text>hi</Text>; }\nexport { Card as default };").unwrap();
        assert!(listed.ends_with("__roxMount(Card);\n"), "{listed}");
        let v0 = build("card", "Card", "import { Text } from \"nsh\";\nfunction R() { return <Text>hi</Text>; }").unwrap();
        assert!(v0.ends_with("__roxMount(R);\n"), "{v0}");
        let none = build("card", "Card", "import { Text } from \"nsh\";\nfunction Card() { return <Text>hi</Text>; }").unwrap_err();
        assert_eq!(none.message, "Could not open Card: it has no default export.");
    }

    #[test]
    fn rox_imports_bind_the_shims_globals() {
        let js = build("card", "Card", "import { rox, RoxError as E } from \"rox\";\nimport { Text } from \"nsh\";\nexport default function Card() { return <Text>{rox.app.app + String(E)}</Text>; }").unwrap();
        assert!(js.contains("const rox = globalThis.rox;const E = globalThis.RoxError;"), "{js}");
        assert!(!js.contains("from \"rox\""), "{js}");
    }

    #[test]
    fn app_08_load_errors_show_the_specs_messages() {
        let e = build("forms", "BIR Forms", "import { View } from \"nsh\";\nexport default function F() {\n  return <View>\n}\n").unwrap_err();
        assert_eq!(e.code, "compile_error");
        assert!(e.message.starts_with("Could not open BIR Forms: "), "{}", e.message);
        assert!(!e.message.contains('\n') && !e.message.contains("return <View>"), "no source shown: {}", e.message);
        assert!(e.message.ends_with("(line 4)."), "{}", e.message);
        use nsh_engine::AppErrorKind as K;
        assert_eq!(runtime_message("Forms", K::Uncaught, "TypeError: x is undefined\n  at f").as_deref(), Some("Forms stopped: TypeError: x is undefined."));
        assert_eq!(runtime_message("Forms", K::Stopped, "out of memory").as_deref(), Some("Forms stopped: it ran out of memory."));
        assert_eq!(runtime_message("Forms", K::Stopped, "InternalError: interrupted").as_deref(), Some("Forms stopped: it took too long."));
        assert_eq!(runtime_message("Forms", K::Asset, "font"), None);
        let long = runtime_message("Forms", K::Uncaught, &"e".repeat(300)).unwrap();
        assert_eq!(long.len(), "Forms stopped: .".len() + 200);
        assert_eq!(say::unavailable("Fixture"), "Fixture is not running. Retry");
    }

    /// A broker listing one plugin whose `card` app is `listed` and
    /// serving `served` as its source.
    struct Serves {
        listed: Value,
        served: String,
    }

    impl host_api::Broker for Serves {
        fn request(&self, _: &str, route: &str, _: Option<Value>, _: std::time::Duration) -> Result<Value, RoxError> {
            match route {
                "/v1/host/plugins" => Ok(serde_json::json!({"plugins": [{
                    "id": "dev.goldcoders.fixture", "short": "fx", "title": "Fixture", "version": "1.0.0",
                    "protocol": "1.0.0", "enabled": true, "generation": 1, "apps": [self.listed],
                }]})),
                "/v1/host/apps/source" => {
                    Ok(serde_json::json!({"source": self.served, "fingerprint": self.listed["fingerprint"]}))
                }
                _ => Err(RoxError::new("not_found", route)),
            }
        }
    }

    const CARD: &str = "import { Text } from \"nsh/base\";\nimport { rox } from \"rox\";\nexport default function Card() { return <Text>{String(rox.props.ticket)}</Text>; }\n";

    fn card(source: &str) -> Value {
        serde_json::json!({
            "id": "card", "title": "Fixture Card", "entry": "rox-app://card", "mime": "text/x-nsh-tsx", "sdk": "1",
            "fingerprint": fingerprint(source), "placement": ["full", "inline"],
            "props": { "type": "object", "properties": { "ticket": { "type": "string" } }, "required": ["ticket"] },
            "permissions": { "tools": ["fx_echo"] }
        })
    }

    const KEY: &str = "v1:dev.goldcoders.fixture/card?props=%7B%22ticket%22%3A%22tkt_x%22%7D";

    #[test]
    fn app_01_a_one_byte_change_refuses_the_app() {
        let ok = prepare(&Serves { listed: card(CARD), served: CARD.to_owned() }, KEY).unwrap();
        assert_eq!(ok.title, "Fixture Card");
        assert_eq!(ok.fingerprint, fingerprint(CARD));
        assert_eq!(ok.permissions.methods(), ["rox.ticket.title", "rox.fit", "rox.call"]);
        assert!(ok.js.starts_with(SHIM) && ok.js.ends_with("__roxMount(Card);\n"));
        assert_eq!(ok.app_info()["sdk"], "1");
        let changed = CARD.replacen("Card", "Cart", 1);
        let e = prepare(&Serves { listed: card(CARD), served: changed }, KEY).unwrap_err();
        assert_eq!((e.code.as_str(), e.message.as_str()), ("fingerprint_mismatch", say::FINGERPRINT));
        // the broker's own refusal shows the same words
        struct Refuses;
        impl host_api::Broker for Refuses {
            fn request(&self, _: &str, route: &str, _: Option<Value>, _: std::time::Duration) -> Result<Value, RoxError> {
                match route {
                    "/v1/host/plugins" => Serves { listed: card(CARD), served: String::new() }.request("GET", route, None, Default::default()),
                    _ => Err(RoxError::new("fingerprint_mismatch", "sha256 differs")),
                }
            }
        }
        assert_eq!(prepare(&Refuses, KEY).unwrap_err().message, say::FINGERPRINT);
        let mut newer = card(CARD);
        newer["sdk"] = "2".into();
        assert_eq!(prepare(&Serves { listed: newer, served: CARD.to_owned() }, KEY).unwrap_err().code, "not_supported");
    }

    #[test]
    fn app_05_an_invalid_prop_set_does_not_open_the_app() {
        let b = Serves { listed: card(CARD), served: CARD.to_owned() };
        for key in [
            "v1:dev.goldcoders.fixture/card",
            "v1:dev.goldcoders.fixture/card?props=%7B%22ticket%22%3A1%7D",
            "v1:dev.goldcoders.fixture/card?props=%7B%22bad-key%22%3A%22x%22%2C%22ticket%22%3A%22t%22%7D",
        ] {
            assert_eq!(prepare(&b, key).unwrap_err().code, "invalid_args", "{key}");
        }
        assert_eq!(prepare(&b, "v1:dev.goldcoders.fixture/nope").unwrap_err().code, "not_found");
        assert_eq!(prepare(&b, "v1:dev.goldcoders.other/card").unwrap_err().code, "not_found");
    }

    #[test]
    fn app_01_fingerprints_cover_the_bytes_as_sent() {
        let src = "export default function A() { return null; }";
        let fp = fingerprint(src);
        assert_eq!(fp, format!("sha256:{}", keys::sha256_hex(src.as_bytes())));
        assert_ne!(fingerprint(&format!("{src} ")), fp);
        assert_ne!(fingerprint(&src.replace('A', "B")), fp);
    }
}
