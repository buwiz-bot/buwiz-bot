//! The remaining leaves: Reminders through JXA, Linear's GraphQL (the key from
//! `LINEAR_API_KEY` or `secret get`), and the file routes that write a
//! reply's file or hand one to another app (`/file/write`, `/file/open`,
//! `/file/save`). The commands go through `LocalTools`, Linear through
//! `helper/http.rs`.

use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Response, StatusCode};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use crate::helper::local::MacTools;
use crate::hub::ctx::HubCtx;
use crate::hub::jsval::{get, parse_error, trim, truthy};
use crate::hub::traits::{LocalCommand, LocalTools};
use crate::hub::types::{self, HubRequest, HubResponse, Pattern, Verb};
use crate::hub::Registry;
use crate::js;

/// Registers the nine routes, and the Mac's local tools unless a test
/// already gave the hub its own.
pub fn install(routes: &mut Registry, ctx: &mut HubCtx) {
    if ctx.local.is_none() {
        ctx.local = Some(Arc::new(MacTools));
    }
    routes.route(Verb::Post, Pattern::Exact("/file/write"), |c, r| exact(c, r, file_write));
    routes.route(Verb::Post, Pattern::Exact("/file/open"), |c, r| exact(c, r, file_open));
    routes.route(Verb::Post, Pattern::Exact("/file/save"), |c, r| exact(c, r, file_save));
}

/// These match on `req.url`: with a query string none matches, and
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

/// What a throw answers before any other answer.
fn thrown(error: &str) -> HubResponse {
    types::json(StatusCode::INTERNAL_SERVER_ERROR, &json!({ "error": error }))
}

// ————— files —————

/// The data dir's folders.
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

/// `path.normalize` (POSIX), as leaf/files.rs has it.
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

/// A file-system error's message: `ENOENT: no such file or directory, open '<p>'`.
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

/// The kinds macOS `open` runs, installs or follows rather than shows: apps and bundles, the
/// scripts Terminal, Script Editor or a launcher runs, Automator actions, installers and disk
/// images, profiles, and links to another file or URL.
const RUNNABLE: [&str; 42] = [
    "app", "command", "tool", "terminal", "sh", "bash", "zsh", "csh", "tcsh", "ksh", "fish", "py", "pyw", "pl", "rb", "php", "scpt",
    "scptd", "applescript", "workflow", "action", "definition", "caction", "shortcut", "pkg", "mpkg", "dmg", "jar", "prefpane", "saver", "plugin",
    "bundle", "kext", "osax", "qlgenerator", "mobileconfig", "fileloc", "inetloc", "webloc", "url", "service", "systemextension",
];

/// Whether `open` on `p` would run something: a folder (an `.app` is one) or a [`RUNNABLE`]
/// kind. Those are only shown in Finder.
fn runs_when_opened(p: &str) -> bool {
    Path::new(p).is_dir() || RUNNABLE.contains(&ext_of_path(p).as_str())
}

/// `POST /file/open {path}`: the file in its own app (`open`, not waited for); a file `open`
/// would run is shown in Finder instead (`open -R`).
async fn file_open(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let dirs = Dirs::of(&ctx);
    let p = body_json(&req).ok().and_then(|b| if b.is_null() { None } else { viewer_of(&ctx, &dirs, get(Some(&b), "path")).ok().flatten() });
    let Some(p) = p.filter(|p| exists(p)) else { return ok(&json!({ "ok": false })) };
    let args = if runs_when_opened(&p) { vec!["-R".to_owned(), p] } else { vec![p] };
    let tools = local(&ctx);
    tokio::spawn(async move {
        let _ = tools.run(LocalCommand { program: "open".into(), args, ..Default::default() }).await;
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
    use crate::hub::sources;
    use crate::hub::sources::reminders::{HINT, REMINDERS_ADD, REMINDERS_COMPLETE, REMINDERS_LIST};
    use crate::hub::traits::{Clock, LocalCommand, LocalOutput, LocalTools};
    use std::sync::Mutex;
    use crate::hub::Hub;
    use hyper::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
    use hyper::Method;

    /// Answers every command with `out` and records it.
    struct Stub {
        out: Mutex<LocalOutput>,
        calls: Mutex<Vec<LocalCommand>>,
        /// What EventKit would say: `None` is "unavailable or denied", so the
        /// route runs osascript. Calls to the EventKit methods are recorded.
        ek: Mutex<Option<Result<Value, String>>>,
        ek_calls: Mutex<Vec<String>>,
    }

    impl LocalTools for Stub {
        fn run(&self, cmd: LocalCommand) -> types::BoxFuture<'static, Result<LocalOutput, String>> {
            self.calls.lock().unwrap().push(cmd);
            let out = self.out.lock().unwrap().clone();
            Box::pin(async move { Ok(out) })
        }

        fn reminders_list(&self) -> types::BoxFuture<'static, Option<Result<Value, String>>> {
            self.ek_calls.lock().unwrap().push("list".into());
            let ek = self.ek.lock().unwrap().clone();
            Box::pin(async move { ek })
        }

        fn reminders_complete(&self, name: String) -> types::BoxFuture<'static, Option<Result<Value, String>>> {
            self.ek_calls.lock().unwrap().push(format!("complete {name}"));
            let ek = self.ek.lock().unwrap().clone();
            Box::pin(async move { ek })
        }

        fn reminders_add(&self, name: String, list: String, due: String) -> types::BoxFuture<'static, Option<Result<Value, String>>> {
            self.ek_calls.lock().unwrap().push(format!("add {name}|{list}|{due}"));
            let ek = self.ek.lock().unwrap().clone();
            Box::pin(async move { ek })
        }
    }

    fn hub(data: &Path, out: LocalOutput) -> (Hub, Arc<Stub>) {
        let stub = Arc::new(Stub { out: Mutex::new(out), calls: Mutex::new(Vec::new()), ek: Mutex::new(None), ek_calls: Mutex::new(Vec::new()) });
        let mut ctx = HubCtx::new(data.to_path_buf(), Clock::manual());
        ctx.local = Some(stub.clone());
        (Hub::with(ctx, Vec::new(), install), stub)
    }

    async fn call(hub: &Hub, method: Method, url: &str, body: &str) -> (u16, String) {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert("x-buwiz-host", HeaderValue::from_static(crate::hub::routes::host::TEST_TOKEN));
        let req = HubRequest { method, url: url.into(), headers, body: Bytes::from(body.to_owned()), params: Vec::new() };
        let res = hub.answer(req).await;
        (res.status().as_u16(), String::from_utf8_lossy(&res.into_body().collect().await.unwrap().to_bytes()).into_owned())
    }

    fn printed(s: &str, code: i32) -> LocalOutput {
        LocalOutput { code: Some(code), stdout: s.as_bytes().to_vec(), stderr: b"execution error\n".to_vec() }
    }

    fn temp(label: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("buwiz-left-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("generated/files/t1")).unwrap();
        d
    }

    /// A hub context whose local tools are the stub.
    fn ctx_with(stub: &Arc<Stub>, label: &str) -> HubCtx {
        let mut ctx = HubCtx::new(temp(label), Clock::manual());
        ctx.local = Some(stub.clone());
        ctx
    }

    fn stub(out: LocalOutput) -> Arc<Stub> {
        Arc::new(Stub { out: Mutex::new(out), calls: Mutex::new(Vec::new()), ek: Mutex::new(None), ek_calls: Mutex::new(Vec::new()) })
    }

    /// What `/data/reminders*` pinned, now through the `reminders` source's own code
    /// (the routes are gone; widgets call the source).
    #[tokio::test]
    async fn left_01b_reminders_use_eventkit_and_fall_back_to_osascript_only_when_it_is_unavailable() {
        let stub = stub(printed("[{\"name\":\"jxa\"}]\n", 0));
        let ctx = ctx_with(&stub, "rem-ek");
        let add = json!({"name": "x", "list": "Home", "due": "2026-11-13"});
        let complete = json!({"name": "x"});
        let all = |ctx: &HubCtx, add: Value, complete: Value| {
            let ctx = ctx as *const HubCtx;
            // the three actions, in the order the widget calls them
            async move {
                let ctx = unsafe { &*ctx };
                [sources::reminders::list(ctx).await, sources::reminders::change(ctx, false, &add).await.unwrap(), sources::reminders::change(ctx, true, &complete).await.unwrap()]
            }
        };
        // a real EventKit result is the body, and osascript does not run
        *stub.ek.lock().unwrap() = Some(Ok(json!({ "ok": true })));
        assert_eq!(all(&ctx, add.clone(), complete.clone()).await, [json!({"ok": true}), json!({"ok": true}), json!({"ok": true})]);
        // a real EventKit error is the error body, and osascript does not run
        *stub.ek.lock().unwrap() = Some(Err("save failed".into()));
        assert_eq!(all(&ctx, add.clone(), complete.clone()).await, [json!({"error": "save failed"}), json!({"error": "save failed"}), json!({"error": "save failed"})]);
        assert!(stub.calls.lock().unwrap().is_empty());
        assert_eq!(stub.ek_calls.lock().unwrap().iter().filter(|c| c.starts_with("add ")).collect::<Vec<_>>(), ["add x|Home|2026-11-13", "add x|Home|2026-11-13"]);
        // unavailable or denied: the old osascript path answers, same shapes
        *stub.ek.lock().unwrap() = None;
        assert_eq!(sources::reminders::list(&ctx).await, json!({"reminders": [{"name": "jxa"}]}));
        *stub.out.lock().unwrap() = printed("{\"ok\":true}\n", 0);
        assert_eq!(sources::reminders::change(&ctx, false, &json!({"name": "x"})).await.unwrap(), json!({"ok": true}));
        assert_eq!(sources::reminders::change(&ctx, true, &json!({"name": "x"})).await.unwrap(), json!({"ok": true}));
        let cmds = stub.calls.lock().unwrap().clone();
        assert_eq!(cmds.len(), 3);
        assert!(cmds.iter().all(|c| c.program == "osascript"));
        assert_eq!(cmds[0].args[3], REMINDERS_LIST);
        assert_eq!(cmds[1].args[3], REMINDERS_ADD);
        assert_eq!(cmds[2].args[3], REMINDERS_COMPLETE);
    }

    #[tokio::test]
    async fn left_01_reminders_wrap_the_jxa_list_and_keep_its_errors() {
        let stub = stub(printed("[{\"name\":\"a\"}]\n", 0));
        let ctx = ctx_with(&stub, "rem");
        assert_eq!(sources::reminders::list(&ctx).await, json!({"reminders": [{"name": "a"}]}));
        let cmd = stub.calls.lock().unwrap()[0].clone();
        assert_eq!((cmd.program.as_str(), &cmd.args[..3], cmd.args[3].as_str()), ("osascript", &["-l".to_owned(), "JavaScript".into(), "-e".into()][..], REMINDERS_LIST));
        *stub.out.lock().unwrap() = printed("", 1);
        let failed = json!({"error": "Command failed: osascript -l JavaScript -e ", "hint": HINT});
        assert_eq!(sources::reminders::list(&ctx).await, failed);
        assert_eq!(sources::reminders::change(&ctx, false, &json!({"name": "x"})).await.unwrap(), failed);
        assert_eq!(stub.calls.lock().unwrap()[2].env, [("REM_NAME".to_owned(), "x".to_owned()), ("REM_LIST".into(), "".into()), ("REM_DUE".into(), "".into())]);
        assert_eq!(sources::reminders::change(&ctx, true, &json!({})).await.unwrap(), json!({"error": "name?"}));
        // a `null` body answers a TypeError
        assert_eq!(sources::reminders::change(&ctx, true, &Value::Null).await.unwrap_err(), "Cannot read properties of null (reading 'name')");
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

    /// A file `open` would run (a script, an app, a link) is only shown in Finder.
    #[tokio::test]
    async fn left_07_file_open_reveals_what_open_would_run() {
        let data = temp("open");
        let (hub, stub) = hub(&data, printed("", 0));
        let files = data.join("generated/files");
        for f in ["doc.pdf", "run.command", "run.COMMAND", "run.terminal", "x.scpt", "go.workflow", "a.fileloc", "s.sh"] {
            std::fs::write(files.join("t1").join(f), "x").unwrap();
        }
        std::fs::create_dir_all(files.join("t1/Evil.app")).unwrap();
        async fn opened(stub: &Stub) -> LocalCommand {
            for _ in 0..200 {
                if let Some(c) = stub.calls.lock().unwrap().pop() {
                    return c;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            panic!("open never ran");
        }
        assert_eq!(call(&hub, Method::POST, "/file/open", r#"{"path":"t1/doc.pdf"}"#).await.1, r#"{"ok":true}"#);
        let c = opened(&stub).await;
        assert_eq!((c.program.as_str(), c.args.clone()), ("open", vec![files.join("t1/doc.pdf").to_string_lossy().into_owned()]));
        for f in ["run.command", "run.COMMAND", "run.terminal", "x.scpt", "go.workflow", "a.fileloc", "s.sh", "Evil.app"] {
            assert_eq!(call(&hub, Method::POST, "/file/open", &json!({ "path": format!("t1/{f}") }).to_string()).await.1, r#"{"ok":true}"#, "{f}");
            let c = opened(&stub).await;
            assert_eq!(c.args, ["-R".to_owned(), files.join("t1").join(f).to_string_lossy().into_owned()], "{f}");
        }
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
