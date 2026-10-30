//! Track L, the leaves no 5.x item names (server/src/routes/data.mjs and
//! files.mjs): Reminders through JXA, Linear's GraphQL (the key from
//! `LINEAR_API_KEY` or `secret get`), and the file routes that write a
//! reply's file or hand one to another app (`/file/write`, `/file/open`,
//! `/file/save`). The commands go through `LocalTools`, Linear through
//! `helper/http.rs`. Pinned by server/test/leftovers.test.mjs (LEFT-xx).

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Response, StatusCode};
use rusqlite::OptionalExtension;
use serde_json::{json, Map, Value};

use crate::helper::http;
use crate::helper::local::MacTools;
use crate::hub::ctx::HubCtx;
use crate::hub::traits::{LocalCommand, LocalTools};
use crate::hub::types::{self, HubRequest, HubResponse, Pattern, Verb};
use crate::hub::Registry;
use crate::js;

const HINT: &str = "grant permission in System Settings → Privacy & Security → Reminders / Automation";

const REMINDERS_LIST: &str = r#"
      const app = Application("Reminders");
      const out = [];
      outer: for (const l of app.lists()) {
        const ln = l.name();
        for (const r of l.reminders.whose({completed: false})()) {
          let due = null, flagged = false;
          try { const d = r.dueDate(); if (d) due = d.toISOString(); } catch (e) {}
          try { flagged = !!r.flagged(); } catch (e) {}
          out.push({ name: r.name(), list: ln, due, flagged });
          if (out.length >= 50) break outer;
        }
      }
      JSON.stringify(out);"#;

const REMINDERS_COMPLETE: &str = r#"
            const app = Application("Reminders");
            ObjC.import("Cocoa");
            const env = $.NSProcessInfo.processInfo.environment;
            const name = ObjC.unwrap(env.objectForKey("REM_NAME"));
            let done = false;
            outer: for (const l of app.lists()) {
              const hits = l.reminders.whose({completed: false, name})();
              if (hits.length) { hits[0].completed = true; done = true; break outer; }
            }
            JSON.stringify({ ok: done });"#;

const REMINDERS_ADD: &str = r#"
            const app = Application("Reminders");
            ObjC.import("Cocoa");
            const env = $.NSProcessInfo.processInfo.environment;
            const get = (k) => { const v = env.objectForKey(k); return v ? ObjC.unwrap(v) : null; };
            const name = get("REM_NAME"), listName = get("REM_LIST"), due = get("REM_DUE");
            let list = null;
            if (listName) { const hits = app.lists.whose({name: listName})(); if (hits.length) list = hits[0]; }
            if (!list) list = app.defaultList();
            const props = { name };
            if (due) props.dueDate = new Date(due);
            list.reminders.push(app.Reminder(props));
            JSON.stringify({ ok: true, name, list: list.name() });"#;

const LINEAR_URL: &str = "https://api.linear.app/graphql";
const L_FIELDS: &str = "nodes { id identifier title priority url updatedAt state { id name } assignee { displayName } team { key } }";
const Q_STATES: &str = "{ teams(first: 10) { nodes { key states { nodes { id name type position } } } } }";
const Q_VIEWER: &str = "{ viewer { id } }";
const Q_UPDATE: &str = "mutation($id: String!, $input: IssueUpdateInput!) { issueUpdate(id: $id, input: $input) { success } }";

/// Registers the nine routes, and the Mac's local tools unless a test
/// already gave the hub its own.
pub fn install(routes: &mut Registry, ctx: &mut HubCtx) {
    if ctx.local.is_none() {
        ctx.local = Some(Arc::new(MacTools));
    }
    routes.route(Verb::Get, Pattern::Exact("/data/reminders"), |c, r| exact(c, r, reminders));
    routes.route(Verb::Post, Pattern::Exact("/data/reminders/add"), |c, r| exact(c, r, reminder_change));
    routes.route(Verb::Post, Pattern::Exact("/data/reminders/complete"), |c, r| exact(c, r, reminder_change));
    routes.route(Verb::Get, Pattern::Exact("/data/linear/issues"), |c, r| exact(c, r, linear_issues));
    routes.route(Verb::Get, Pattern::Exact("/data/linear/states"), |c, r| exact(c, r, linear_states));
    routes.route(Verb::Post, Pattern::Exact("/data/linear/assign"), |c, r| exact(c, r, linear_assign));
    routes.route(Verb::Post, Pattern::Exact("/file/write"), |c, r| exact(c, r, file_write));
    routes.route(Verb::Post, Pattern::Exact("/file/open"), |c, r| exact(c, r, file_open));
    routes.route(Verb::Post, Pattern::Exact("/file/save"), |c, r| exact(c, r, file_save));
}

/// Node matches these on `req.url`: with a query string none matches, and
/// its dispatcher ends in a bare 404.
async fn exact<F, Fut>(ctx: Arc<HubCtx>, req: HubRequest, f: F) -> HubResponse
where
    F: FnOnce(Arc<HubCtx>, HubRequest) -> Fut,
    Fut: std::future::Future<Output = HubResponse>,
{
    if req.url != req.path() {
        let mut res = Response::new(Full::new(Bytes::from_static(b"not found")).map_err(|never| match never {}).boxed());
        *res.status_mut() = StatusCode::NOT_FOUND;
        return res;
    }
    f(ctx, req).await
}

fn ok(v: &Value) -> HubResponse {
    types::json(StatusCode::OK, v)
}

fn local(ctx: &HubCtx) -> Arc<dyn LocalTools> {
    ctx.local.clone().expect("leftovers::install gives the hub its local tools")
}

/// `JSON.parse(body || "{}")`, or the SyntaxError's message.
fn body_json(req: &HubRequest) -> Result<Value, String> {
    let text = String::from_utf8_lossy(&req.body);
    let text = if text.is_empty() { "{}" } else { &text };
    serde_json::from_str(text).map_err(|e| parse_error(text, &e))
}

/// The JSON body Node reads with `try { b = JSON.parse(…) } catch {}`:
/// `{}` when it does not parse.
fn body_or_empty(req: &HubRequest) -> Value {
    body_json(req).unwrap_or_else(|_| json!({}))
}

/// `b.<name>` of a parsed body; `Err` with the TypeError Node throws when
/// the body is `null`.
fn field<'a>(b: &'a Value, name: &str) -> Result<Option<&'a Value>, String> {
    match b {
        Value::Null => Err(format!("Cannot read properties of null (reading '{name}')")),
        Value::Object(m) => Ok(m.get(name)),
        _ => Ok(None),
    }
}

/// What Node's dispatcher answers for a throw before any answer (there, a
/// throw inside a body's `end` handler is uncaught and ends the process).
fn thrown(error: &str) -> HubResponse {
    types::json(StatusCode::INTERNAL_SERVER_ERROR, &json!({ "error": error }))
}

/// `a?.b`: the member of an object, else nothing.
fn get<'a>(v: Option<&'a Value>, name: &str) -> Option<&'a Value> {
    v.and_then(|v| v.as_object()).and_then(|m| m.get(name))
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(js::truthy)
}

/// `s.trim()`.
fn trim(s: &str) -> &str {
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

// ————— Reminders —————

/// `jxa(script, env)` (state.mjs): `osascript -l JavaScript -e <script>`
/// with a 15 s timeout, its output parsed as JSON, or `{error, hint}`.
async fn jxa(ctx: &HubCtx, script: &str, env: Vec<(String, String)>) -> Value {
    let cmd = LocalCommand { program: "osascript".into(), args: vec!["-l".into(), "JavaScript".into(), "-e".into(), script.into()], env, stdin: None, timeout: Some(Duration::from_secs(15)) };
    let failed = |message: String| json!({ "error": message.split('\n').next().unwrap_or(""), "hint": HINT });
    match local(ctx).run(cmd).await {
        Err(code) => failed(format!("spawnSync osascript {code}")),
        Ok(out) if out.code.is_none() => failed("spawnSync osascript ETIMEDOUT".into()),
        Ok(out) if out.code != Some(0) => failed(format!("Command failed: osascript -l JavaScript -e {script}\n{}", String::from_utf8_lossy(&out.stderr))),
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let text = trim(&text);
            let text = if text.is_empty() { "null" } else { text };
            serde_json::from_str(text).unwrap_or_else(|e| failed(parse_error(text, &e)))
        }
    }
}

/// `GET /data/reminders`: `{reminders}`, or the script's `{error}` as is.
async fn reminders(ctx: Arc<HubCtx>, _req: HubRequest) -> HubResponse {
    let out = jxa(&ctx, REMINDERS_LIST, Vec::new()).await;
    if truthy(get(Some(&out), "error")) {
        return ok(&out);
    }
    ok(&json!({ "reminders": if js::truthy(&out) { out } else { json!([]) } }))
}

/// `POST /data/reminders/add` and `/complete` `{name, list?, due?}`.
async fn reminder_change(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let b = body_or_empty(&req);
    let mut env = Vec::new();
    for (key, name) in [("REM_NAME", "name"), ("REM_LIST", "list"), ("REM_DUE", "due")] {
        let v = match field(&b, name) {
            Ok(v) => v,
            Err(e) => return thrown(&e),
        };
        env.push((key.to_owned(), if truthy(v) { js::string_of(v.expect("truthy")) } else { String::new() }));
    }
    if env[0].1.is_empty() {
        return ok(&json!({ "error": "name?" }));
    }
    let script = if req.path() == "/data/reminders/complete" { REMINDERS_COMPLETE } else { REMINDERS_ADD };
    ok(&jxa(&ctx, script, env).await)
}

// ————— Linear —————

/// The key once found (Node's `linearKey`); a miss is looked up again.
static LINEAR_KEY: Mutex<Option<String>> = Mutex::new(None);

/// `getLinearKey()`: `LINEAR_API_KEY`, else `secret get LINEAR_API_KEY`.
pub(crate) async fn linear_key(ctx: &HubCtx) -> Option<String> {
    if let Some(k) = LINEAR_KEY.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Some(k);
    }
    let mut key = std::env::var("LINEAR_API_KEY").ok().filter(|k| !k.is_empty());
    if key.is_none() {
        let cmd = LocalCommand { program: "secret".into(), args: vec!["get".into(), "LINEAR_API_KEY".into()], ..Default::default() };
        if let Ok(out) = local(ctx).run(cmd).await {
            if out.code == Some(0) {
                key = Some(trim(&String::from_utf8_lossy(&out.stdout)).to_owned()).filter(|k| !k.is_empty());
            }
        }
    }
    if let Some(k) = &key {
        *LINEAR_KEY.lock().unwrap_or_else(|e| e.into_inner()) = Some(k.clone());
    }
    key
}

/// `lgql(query, variables)`: the response's JSON, `{_nokey: true}` with no
/// key, or the message fetch or `r.json()` rejects with.
async fn lgql(ctx: &HubCtx, query: &str, variables: Option<Value>) -> Result<Value, String> {
    let Some(key) = linear_key(ctx).await else { return Ok(json!({ "_nokey": true })) };
    let body = match variables {
        Some(v) => json!({ "query": query, "variables": v }),
        None => json!({ "query": query }),
    };
    let url = url::Url::parse(LINEAR_URL).expect("a fixed URL");
    let req = http::Req::post(url, js::stringify(&body).into_bytes()).header("Authorization", &key).header("Content-Type", "application/json");
    let res = http::fetch(req).await.map_err(|_| "fetch failed".to_owned())?;
    let text = res.text().await.map_err(|_| "terminated".to_owned())?;
    serde_json::from_str(&text).map_err(|e| parse_error(&text, &e))
}

/// `throw new Error(String(d.errors[0]?.message))`, caught as
/// `String(e?.message || e)`.
fn graphql_error(d: &Value) -> Option<String> {
    let errors = get(Some(d), "errors");
    if !truthy(errors) {
        return None;
    }
    let first = errors.and_then(|e| e.as_array()).and_then(|a| a.first());
    let msg = get(first, "message").map(js::string_of).unwrap_or_else(|| "undefined".to_owned());
    Some(if msg.is_empty() { "Error".to_owned() } else { msg })
}

fn bad_gateway(error: &str) -> HubResponse {
    types::json(StatusCode::BAD_GATEWAY, &json!({ "error": error }))
}

/// `a.b?.c || fallback`.
fn or(v: Option<&Value>, fallback: Value) -> Value {
    if truthy(v) {
        v.cloned().expect("truthy")
    } else {
        fallback
    }
}

/// An issue as the widget gets it (keys of missing members left out, as
/// JSON.stringify drops `undefined`).
fn issue(n: &Value) -> Value {
    let mut m = Map::new();
    let mut put = |k: &str, v: Option<&Value>| {
        if let Some(v) = v {
            m.insert(k.to_owned(), v.clone());
        }
    };
    put("uid", get(Some(n), "id"));
    put("id", get(Some(n), "identifier"));
    put("title", get(Some(n), "title"));
    m.insert("state".into(), or(get(get(Some(n), "state"), "name"), json!("")));
    m.insert("stateId".into(), or(get(get(Some(n), "state"), "id"), json!("")));
    if let Some(p) = get(Some(n), "priority") {
        m.insert("priority".into(), p.clone());
    }
    m.insert("assignee".into(), or(get(get(Some(n), "assignee"), "displayName"), Value::Null));
    m.insert("team".into(), or(get(get(Some(n), "team"), "key"), Value::Null));
    if let Some(u) = get(Some(n), "url") {
        m.insert("url".into(), u.clone());
    }
    Value::Object(m)
}

fn shape(nodes: &[Value], scope: &str) -> Value {
    json!({ "scope": scope, "issues": nodes.iter().map(issue).collect::<Vec<_>>() })
}

/// `x || []` of a nodes list; a list only.
fn nodes(v: Option<&Value>) -> Vec<Value> {
    v.and_then(|v| v.as_array()).cloned().unwrap_or_default()
}

/// `GET /data/linear/issues`: assigned to me, else the workspace's latest.
async fn linear_issues(ctx: Arc<HubCtx>, _req: HubRequest) -> HubResponse {
    let filter = r#"filter: { state: { type: { nin: ["completed","canceled"] } } }"#;
    let d = match lgql(&ctx, &format!("{{ viewer {{ assignedIssues(first: 25, {filter}) {{ {L_FIELDS} }} }} }}"), None).await {
        Ok(d) => d,
        Err(e) => return bad_gateway(&e),
    };
    if truthy(get(Some(&d), "_nokey")) {
        return ok(&json!({ "error": "linear not configured — run `secret set LINEAR_API_KEY`" }));
    }
    if let Some(e) = graphql_error(&d) {
        return bad_gateway(&e);
    }
    let assigned = nodes(get(get(get(get(Some(&d), "data"), "viewer"), "assignedIssues"), "nodes"));
    if !assigned.is_empty() {
        return ok(&shape(&assigned, "assigned"));
    }
    let d2 = match lgql(&ctx, &format!("{{ issues(first: 25, orderBy: updatedAt, {filter}) {{ {L_FIELDS} }} }}"), None).await {
        Ok(d) => d,
        Err(e) => return bad_gateway(&e),
    };
    if let Some(e) = graphql_error(&d2) {
        return bad_gateway(&e);
    }
    ok(&shape(&nodes(get(get(get(Some(&d2), "data"), "issues"), "nodes")), "workspace"))
}

/// `GET /data/linear/states`: every team's workflow states, by position.
async fn linear_states(ctx: Arc<HubCtx>, _req: HubRequest) -> HubResponse {
    let d = match lgql(&ctx, Q_STATES, None).await {
        Ok(d) => d,
        Err(e) => return bad_gateway(&e),
    };
    if truthy(get(Some(&d), "_nokey")) {
        return ok(&json!({ "error": "linear not configured" }));
    }
    if let Some(e) = graphql_error(&d) {
        return bad_gateway(&e);
    }
    let position = |s: &Value| get(Some(s), "position").and_then(Value::as_f64).unwrap_or(f64::NAN);
    let teams: Vec<Value> = nodes(get(get(get(Some(&d), "data"), "teams"), "nodes"))
        .iter()
        .map(|t| {
            let mut states = nodes(get(get(Some(t), "states"), "nodes"));
            states.sort_by(|a, b| (position(a) - position(b)).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut m = Map::new();
            if let Some(k) = get(Some(t), "key") {
                m.insert("team".into(), k.clone());
            }
            let states = states
                .iter()
                .map(|s| {
                    let mut m = Map::new();
                    for k in ["id", "name", "type"] {
                        if let Some(v) = get(Some(s), k) {
                            m.insert(k.into(), v.clone());
                        }
                    }
                    Value::Object(m)
                })
                .collect();
            m.insert("states".into(), Value::Array(states));
            Value::Object(m)
        })
        .collect();
    ok(&json!({ "teams": teams }))
}

/// `POST /data/linear/assign {uid}`: the issue to the key's owner. Every
/// answer here is a 200.
async fn linear_assign(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let b = body_or_empty(&req);
    let uid = match field(&b, "uid") {
        Ok(v) => v.cloned(),
        Err(e) => return thrown(&e),
    };
    if !truthy(uid.as_ref()) {
        return ok(&json!({ "error": "uid?" }));
    }
    let run = async {
        let d = lgql(&ctx, Q_VIEWER, None).await?;
        let vid = get(get(get(Some(&d), "data"), "viewer"), "id").filter(|v| js::truthy(v)).cloned().ok_or_else(|| "no viewer".to_owned())?;
        let d2 = lgql(&ctx, Q_UPDATE, Some(json!({ "id": uid, "input": { "assigneeId": vid } }))).await?;
        match graphql_error(&d2) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    };
    match run.await {
        Ok(()) => ok(&json!({ "ok": true })),
        Err(e) => ok(&json!({ "error": e })),
    }
}

// ————— files —————

/// The data dir's folders, as config.mjs joins them.
struct Dirs {
    data: String,
    gen: String,
    out: String,
}

impl Dirs {
    fn of(ctx: &HubCtx) -> Self {
        let data = normalize(&ctx.data.to_string_lossy());
        let gen = join(&data, "generated");
        let out = join(&gen, "files");
        Self { data, gen, out }
    }

    /// `outFile(rel)`: a path under the files folder, else `None`.
    fn out_file(&self, rel: &str) -> Option<String> {
        let p = join(&self.out, rel);
        p.starts_with(&format!("{}/", self.out)).then_some(p)
    }
}

/// Node's `path.normalize` (POSIX), as leaf/files.rs has it.
fn normalize(path: &str) -> String {
    let absolute = path.starts_with('/');
    let trailing = path.ends_with('/');
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|l| *l != "..") {
                    out.pop();
                } else if !absolute {
                    out.push("..");
                }
            }
            seg => out.push(seg),
        }
    }
    let mut s = out.join("/");
    if s.is_empty() && !absolute {
        s.push('.');
    }
    if trailing && !s.is_empty() {
        s.push('/');
    }
    if absolute {
        format!("/{s}")
    } else {
        s
    }
}

/// `path.join(dir, name)`.
fn join(dir: &str, name: &str) -> String {
    if name.is_empty() {
        return normalize(dir);
    }
    normalize(&format!("{dir}/{name}"))
}

/// `path.dirname(p)` of an absolute path.
fn dirname(p: &str) -> String {
    let p = if p.len() > 1 { p.trim_end_matches('/') } else { p };
    match p.rsplit_once('/') {
        Some(("", _)) | None => "/".to_owned(),
        Some((dir, _)) => dir.to_owned(),
    }
}

fn exists(p: &str) -> bool {
    Path::new(p).exists()
}

/// `versionOf(path)?.rel`: the original file of a `.versions/<rel>/v<n>.<ext>`.
fn version_of(path: &str) -> Option<String> {
    let rest = path.get(..10).filter(|p| p.eq_ignore_ascii_case(".versions/")).map(|_| &path[10..])?;
    let (rel, file) = rest.rsplit_once('/')?;
    let (stem, ext) = file.split_once('.')?;
    let digits = stem.strip_prefix(['v', 'V'])?;
    let ok = !rel.is_empty() && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && !ext.is_empty() && ext.bytes().all(|b| b.is_ascii_alphanumeric());
    ok.then(|| rel.to_owned())
}

/// `/\.([a-z0-9]+)$/i` of a path, lower-cased, else `bin`.
fn ext_of_path(rel: &str) -> String {
    match rel.rsplit_once('.') {
        Some((_, e)) if !e.is_empty() && e.bytes().all(|b| b.is_ascii_alphanumeric()) => e.to_ascii_lowercase(),
        _ => "bin".to_owned(),
    }
}

/// `Buffer.from(s, "base64")`: both alphabets, other characters skipped,
/// stopping at the first `=`, a partial last byte dropped.
pub fn base64_loose(s: &str) -> Vec<u8> {
    let (mut acc, mut bits, mut out) = (0u32, 0u32, Vec::new());
    for b in s.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => continue,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

/// A Node fs error's message: `ENOENT: no such file or directory, open '<p>'`.
fn fs_error(e: &std::io::Error, syscall: &str, path: &str, dest: Option<&str>) -> String {
    let (code, desc) = match e.raw_os_error() {
        Some(libc::ENOENT) => ("ENOENT", "no such file or directory"),
        Some(libc::EISDIR) => ("EISDIR", "illegal operation on a directory"),
        Some(libc::ENOTDIR) => ("ENOTDIR", "not a directory"),
        Some(libc::EACCES) => ("EACCES", "permission denied"),
        Some(libc::EPERM) => ("EPERM", "operation not permitted"),
        Some(libc::EEXIST) => ("EEXIST", "file already exists"),
        Some(libc::ENOSPC) => ("ENOSPC", "no space left on device"),
        Some(libc::EROFS) => ("EROFS", "read-only file system"),
        Some(libc::ENOTSUP) => ("ENOTSUP", "operation not supported on socket"),
        _ => return e.to_string(),
    };
    match dest {
        Some(d) => format!("{code}: {desc}, {syscall} '{path}' -> '{d}'"),
        None => format!("{code}: {desc}, {syscall} '{path}'"),
    }
}

/// `copyFileSync(src, dest)`: a folder can't be copied (macOS's copyfile
/// says ENOTSUP).
fn copy_file(src: &str, dest: &str) -> Result<(), String> {
    if Path::new(src).is_dir() {
        return Err(fs_error(&std::io::Error::from_raw_os_error(libc::ENOTSUP), "copyfile", src, Some(dest)));
    }
    std::fs::copy(src, dest).map(|_| ()).map_err(|e| fs_error(&e, "copyfile", src, Some(dest)))
}

fn write_file(p: &str, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(p, bytes).map_err(|e| fs_error(&e, "open", p, None))
}

/// The newest of `versionsOfFile(rel)` (its number), and how many there are.
fn versions(dirs: &Dirs, rel: &str) -> Result<(usize, f64), String> {
    let Some(dir) = dirs.out_file(&format!(".versions/{rel}")) else { return Ok((0, 0.0)) };
    if !exists(&dir) {
        return Ok((0, 0.0));
    }
    let read = std::fs::read_dir(&dir).map_err(|e| fs_error(&e, "scandir", &dir, None))?;
    let mut count = 0;
    let mut last: f64 = 0.0;
    for entry in read.flatten() {
        let f = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = f.strip_prefix('v') else { continue };
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() || rest.as_bytes().get(digits.len()) != Some(&b'.') {
            continue;
        }
        count += 1;
        last = last.max(digits.parse::<f64>().unwrap_or(f64::INFINITY));
    }
    Ok((count, last))
}

/// `keepVersion(rel, p, bytes)`: writes the file as its next version (the
/// content before its first save kept as v1); that version's number.
fn keep_version(dirs: &Dirs, rel: &str, p: &str, bytes: &[u8]) -> Result<f64, String> {
    let dir = dirs.out_file(&format!(".versions/{rel}"));
    let ext = ext_of_path(rel);
    let mut vs = versions(dirs, rel)?;
    if let Some(dir) = &dir {
        std::fs::create_dir_all(dir).map_err(|e| fs_error(&e, "mkdir", dir, None))?;
        if vs.0 == 0 && exists(p) {
            copy_file(p, &format!("{dir}/v1.{ext}"))?;
            vs = versions(dirs, rel)?;
        }
    }
    write_file(p, bytes)?;
    let v = vs.1 + 1.0;
    if let Some(dir) = &dir {
        write_file(&format!("{dir}/v{}.{ext}", js::number(v)), bytes)?;
    }
    Ok(v)
}

/// `path.join`'s TypeError for a path that is not a string.
fn not_a_path(v: &Value) -> String {
    let got = match v {
        Value::Number(n) => format!("type number ({})", js::number(n.as_f64().unwrap_or(f64::NAN))),
        Value::Bool(b) => format!("type boolean ({b})"),
        Value::Array(_) => "an instance of Array".to_owned(),
        _ => "an instance of Object".to_owned(),
    };
    format!("The \"path\" argument must be of type string. Received {got}")
}

/// `POST /file/write {path, data}`: the Office editor's save, in place.
async fn file_write(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let dirs = Dirs::of(&ctx);
    let answer = (|| -> Result<Value, String> {
        let b = body_json(&req)?;
        if b.is_null() {
            return Err("Cannot destructure property 'path' of 'JSON.parse(...)' as it is null.".to_owned());
        }
        let path = get(Some(&b), "path");
        let data = get(Some(&b), "data");
        // an old version saved again becomes the file's next version
        let rel = match path.filter(|p| js::truthy(p)) {
            None => String::new(),
            Some(p) => match version_of(&js::string_of(p)) {
                Some(rel) if !rel.is_empty() => rel,
                _ => match p {
                    Value::String(s) => s.clone(),
                    other => return Err(not_a_path(other)),
                },
            },
        };
        let Some(p) = dirs.out_file(&rel).filter(|p| exists(&dirname(p))) else { return Ok(json!({ "ok": false, "error": "bad path" })) };
        let bytes = base64_loose(&if truthy(data) { js::string_of(data.expect("truthy")) } else { String::new() });
        let v = keep_version(&dirs, &rel, &p, &bytes)?;
        Ok(json!({ "ok": true, "v": v }))
    })();
    ok(&answer.unwrap_or_else(|e| json!({ "ok": false, "error": js::slice_utf16(&e, 200) })))
}

/// `verFile(name, v)`: the version's file in `generated/<name>/`.
fn ver_file(dirs: &Dirs, name: &str, v: f64) -> Option<String> {
    let dir = join(&dirs.gen, name);
    let mut names: Vec<String> = std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    let suffix = format!(".v{}.tsx", js::number(v));
    names.into_iter().find(|f| f.ends_with(&suffix)).map(|f| join(&dir, &f))
}

/// `/^app:(R\d+)(?:@v(\d+))?$/`: an app's name and its pinned version.
fn app_ref(path: &str) -> Option<(String, Option<&str>)> {
    let rest = path.strip_prefix("app:R")?;
    let (n, v) = match rest.split_once("@v") {
        Some((n, v)) => (n, Some(v)),
        None => (rest, None),
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(n) && v.is_none_or(digits)).then(|| (format!("R{n}"), v))
}

/// `viewerFile(path)`: a reply's file, or an app's source (`app:R<n>`, the
/// current version; `app:R<n>@v<k>`, a pinned one).
fn viewer_file(ctx: &HubCtx, dirs: &Dirs, path: &str) -> Result<Option<String>, String> {
    let Some((name, v)) = app_ref(path) else { return Ok(dirs.out_file(path)) };
    if let Some(v) = v {
        return Ok(ver_file(dirs, &name, v.parse().unwrap_or(f64::INFINITY)));
    }
    let head: Option<i64> = ctx.db.with(|c| c.query_row("SELECT v FROM heads WHERE name=?", [&name], |r| r.get(0)).optional())?;
    let candidates =
        [head.filter(|v| *v != 0).and_then(|v| ver_file(dirs, &name, v as f64)), Some(join(&dirs.gen, &format!("{name}.tsx"))), Some(join(&join(&dirs.data, "widgets"), &format!("{name}.tsx")))];
    Ok(candidates.into_iter().flatten().find(|p| exists(p)))
}

/// `viewerFile(String(x))` of a body's member, as `x || ""`; a non-string
/// path throws in `path.join`.
fn viewer_of(ctx: &HubCtx, dirs: &Dirs, path: Option<&Value>) -> Result<Option<String>, String> {
    match path.filter(|p| js::truthy(p)) {
        None => viewer_file(ctx, dirs, ""),
        Some(Value::String(s)) => viewer_file(ctx, dirs, s),
        Some(other) => {
            let s = js::string_of(other);
            if app_ref(&s).is_some() {
                viewer_file(ctx, dirs, &s)
            } else {
                Err(not_a_path(other))
            }
        }
    }
}

/// `POST /file/open {path}`: the file in its own app (`open`, not waited for).
async fn file_open(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let dirs = Dirs::of(&ctx);
    let p = body_json(&req).ok().and_then(|b| if b.is_null() { None } else { viewer_of(&ctx, &dirs, get(Some(&b), "path")).ok().flatten() });
    let Some(p) = p.filter(|p| exists(p)) else { return ok(&json!({ "ok": false })) };
    let tools = local(&ctx);
    tokio::spawn(async move {
        let _ = tools.run(LocalCommand { program: "open".into(), args: vec![p], ..Default::default() }).await;
    });
    ok(&json!({ "ok": true }))
}

/// `POST /file/save {path | asset, name?}`: the save panel, then a copy.
async fn file_save(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let dirs = Dirs::of(&ctx);
    let b = match body_json(&req) {
        Ok(Value::Null) => return thrown("Cannot destructure property 'path' of 'JSON.parse(...)' as it is null."),
        Ok(b) => b,
        Err(e) => return thrown(&e),
    };
    let (path, asset, name) = (get(Some(&b), "path"), get(Some(&b), "asset"), get(Some(&b), "name"));
    let p = if truthy(asset) {
        let s = js::string_of(asset.expect("truthy"));
        let bare = s.rsplit('/').next().unwrap_or("").to_owned();
        (!bare.is_empty()).then(|| join(&join(&dirs.gen, "assets"), &bare))
    } else {
        match viewer_of(&ctx, &dirs, path) {
            Ok(p) => p,
            Err(e) => return thrown(&e),
        }
    };
    let Some(p) = p.filter(|p| exists(p)) else { return ok(&json!({ "ok": false })) };
    let name = if truthy(name) { js::string_of(name.expect("truthy")) } else { p.rsplit('/').next().unwrap_or("").to_owned() };
    let name: String = name.rsplit('/').next().unwrap_or("").chars().filter(|c| *c != '"' && *c != '\\').collect();
    let script = format!("POSIX path of (choose file name with prompt \"Download\" default name \"{name}\")");
    let picked = local(&ctx).run(LocalCommand { program: "osascript".into(), args: vec!["-e".into(), script], ..Default::default() }).await;
    let dest = match picked {
        Ok(out) if out.code == Some(0) => trim(&String::from_utf8_lossy(&out.stdout)).to_owned(),
        _ => return ok(&json!({ "ok": false, "cancelled": true })),
    };
    match copy_file(&p, &dest) {
        Ok(()) => ok(&json!({ "ok": true, "dest": dest })),
        Err(_) => ok(&json!({ "ok": false, "cancelled": true })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::traits::{Clock, LocalOutput};
    use crate::hub::Hub;
    use hyper::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
    use hyper::Method;

    /// Answers every command with `out` and records it.
    struct Stub {
        out: Mutex<LocalOutput>,
        calls: Mutex<Vec<LocalCommand>>,
    }

    impl LocalTools for Stub {
        fn run(&self, cmd: LocalCommand) -> types::BoxFuture<'static, Result<LocalOutput, String>> {
            self.calls.lock().unwrap().push(cmd);
            let out = self.out.lock().unwrap().clone();
            Box::pin(async move { Ok(out) })
        }
    }

    fn hub(data: &Path, out: LocalOutput) -> (Hub, Arc<Stub>) {
        let stub = Arc::new(Stub { out: Mutex::new(out), calls: Mutex::new(Vec::new()) });
        let mut ctx = HubCtx::new(data.to_path_buf(), Clock::manual());
        ctx.local = Some(stub.clone());
        (Hub::with(ctx, Vec::new(), install), stub)
    }

    async fn call(hub: &Hub, method: Method, url: &str, body: &str) -> (u16, String) {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let req = HubRequest { method, url: url.into(), headers, body: Bytes::from(body.to_owned()), params: Vec::new() };
        let res = hub.answer(req).await;
        (res.status().as_u16(), String::from_utf8_lossy(&res.into_body().collect().await.unwrap().to_bytes()).into_owned())
    }

    fn printed(s: &str, code: i32) -> LocalOutput {
        LocalOutput { code: Some(code), stdout: s.as_bytes().to_vec(), stderr: b"execution error\n".to_vec() }
    }

    fn temp(label: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rox-left-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("generated/files/t1")).unwrap();
        d
    }

    #[tokio::test]
    async fn left_01_reminders_wrap_the_jxa_list_and_keep_its_errors() {
        let data = temp("rem");
        let (hub, stub) = hub(&data, printed("[{\"name\":\"a\"}]\n", 0));
        assert_eq!(call(&hub, Method::GET, "/data/reminders", "").await, (200, r#"{"reminders":[{"name":"a"}]}"#.into()));
        let cmd = stub.calls.lock().unwrap()[0].clone();
        assert_eq!((cmd.program.as_str(), &cmd.args[..3], cmd.args[3].as_str()), ("osascript", &["-l".to_owned(), "JavaScript".into(), "-e".into()][..], REMINDERS_LIST));
        *stub.out.lock().unwrap() = printed("", 1);
        let failed = format!(r#"{{"error":"Command failed: osascript -l JavaScript -e ","hint":"{HINT}"}}"#);
        assert_eq!(call(&hub, Method::GET, "/data/reminders", "").await, (200, failed.clone()));
        assert_eq!(call(&hub, Method::POST, "/data/reminders/add", r#"{"name":"x"}"#).await, (200, failed));
        assert_eq!(stub.calls.lock().unwrap()[2].env, [("REM_NAME".to_owned(), "x".to_owned()), ("REM_LIST".into(), "".into()), ("REM_DUE".into(), "".into())]);
        assert_eq!(call(&hub, Method::POST, "/data/reminders/complete", "nope").await, (200, r#"{"error":"name?"}"#.into()));
        assert_eq!(call(&hub, Method::GET, "/data/reminders?x", "").await, (404, "not found".into()));
        let _ = std::fs::remove_dir_all(data);
    }

    #[tokio::test]
    async fn left_06_file_write_keeps_versions_and_stays_in_the_files_folder() {
        let data = temp("write");
        let (hub, _) = hub(&data, printed("", 0));
        let files = data.join("generated/files");
        std::fs::write(files.join("t1/a.TXT"), "before").unwrap();
        assert_eq!(call(&hub, Method::POST, "/file/write", r#"{"path":"t1/a.TXT","data":"YWZ0ZXI="}"#).await.1, r#"{"ok":true,"v":2}"#);
        assert_eq!(std::fs::read_to_string(files.join(".versions/t1/a.TXT/v1.txt")).unwrap(), "before");
        assert_eq!(std::fs::read_to_string(files.join("t1/a.TXT")).unwrap(), "after");
        for path in ["../x.txt", "t1/../../x.txt", "", "nodir/x"] {
            assert_eq!(call(&hub, Method::POST, "/file/write", &json!({ "path": path }).to_string()).await.1, r#"{"ok":false,"error":"bad path"}"#, "{path}");
        }
        assert_eq!(call(&hub, Method::POST, "/file/write", "nope").await.1, r#"{"ok":false,"error":"Unexpected token 'o', \"nope\" is not valid JSON"}"#);
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn left_helpers_match_node() {
        assert_eq!(base64_loose("_-8\n=Zm9v"), [0xff, 0xef]);
        assert_eq!(base64_loose("a b c d"), [105, 183, 29]);
        assert_eq!(base64_loose("Zm9v!!YmFy"), b"foobar");
        assert_eq!(base64_loose("Zh"), [102]);
        assert!(base64_loose("a").is_empty());
        assert_eq!(version_of(".versions/t1/a.docx/v12.docx"), Some("t1/a.docx".into()));
        assert_eq!(version_of(".Versions/a/V1.X"), Some("a".into()));
        assert_eq!(version_of(".versions/a/v1.tar.gz"), None);
        assert_eq!(ext_of_path("t1/a.DOCX"), "docx");
        assert_eq!(ext_of_path("a.b/c"), "bin");
        assert_eq!(dirname("/a/b/"), "/a");
        assert_eq!(dirname("/a"), "/");
        let err = |s: &str| parse_error(s, &serde_json::from_str::<Value>(s).unwrap_err());
        assert_eq!(err("nope"), "Unexpected token 'o', \"nope\" is not valid JSON");
        assert_eq!(err("{\"a\":}"), "Unexpected token '}', \"{\"a\":}\" is not valid JSON");
        assert_eq!(err("[1,]"), "Unexpected token ']', \"[1,]\" is not valid JSON");
        assert_eq!(err("tru"), "Unexpected end of JSON input");
    }
}
