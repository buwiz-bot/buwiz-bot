//! Pop-out windows (plan 6.3; `POST /popout` in routes/apps.mjs and
//! windows.mjs): a module, or a pinned version of it, in a window of its
//! own. The route writes a mount wrapper (`windows/pop-<name>[-<v>].tsx`),
//! builds it once with `nsh build -o` (the bundle is cached by the
//! wrapper's sha1) and starts `nsh-macos-host` on the bundle, detached.
//! One window per key (`R1`, `R1@v2`): a second press raises the open one,
//! a newer head replaces it. `/app/version` reopens an open window on the
//! new head and the `close_app` tool ends an app's windows, both through
//! [`Windows`], the table Node kept in `openWindows`.
//!
//! AeroSpace is kept (6.3): Rox's "Open in Window" relies on it. It floats
//! a new pop-out (AeroSpace tiles every new window) and raising an open
//! one is AeroSpace-only, as in windows.mjs; without AeroSpace each call
//! fails and is ignored. Pinned by server/test/popout.test.mjs (POP-xx).

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};

use super::chat::{answer, parse};
use crate::hub::ctx::HubCtx;
use crate::hub::library::appmeta::js_round;
use crate::hub::pipeline::versions::{head_of, ver_file};
use crate::hub::pipeline::Dirs;
use crate::hub::store::json::{js_number, js_number_str, out};
use crate::hub::types::{HubRequest, HubResponse, Pattern, Verb};
use crate::hub::Registry;
use crate::js;

/// The wrapper's text, copied from routes/apps.mjs, with its `${…}`s.
const WRAPPER: &str = include_str!("popout_wrapper.tsx");

/// An open window: Node's child process with the fields it hung on it.
struct Win {
    id: u64,
    pid: u32,
    shown_v: f64,
    hint_w: f64,
    hint_h: f64,
    /// `child.kill()` was sent
    killed: bool,
    /// `child.exitCode !== null`
    exited: Arc<AtomicBool>,
}

impl Win {
    fn open(&self) -> bool {
        !self.exited.load(Ordering::SeqCst) && !self.killed
    }

    /// `child.kill()`: SIGTERM, unless it already exited.
    fn kill(&mut self) {
        if !self.exited.load(Ordering::SeqCst) {
            unsafe { libc::kill(self.pid as i32, libc::SIGTERM) };
        }
        self.killed = true;
    }
}

/// The open pop-outs by key (`openWindows`), in the hub's `ext`.
#[derive(Default)]
pub struct Windows {
    open: Mutex<HashMap<String, Win>>,
    next: AtomicU64,
    /// Node's handler ran synchronously from the wrapper to the spawn: one
    /// press at a time
    serial: tokio::sync::Mutex<()>,
}

impl Windows {
    fn table(&self) -> std::sync::MutexGuard<'_, HashMap<String, Win>> {
        self.open.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn windows(ctx: &HubCtx) -> Option<Arc<Windows>> {
    ctx.ext.get::<Arc<Windows>>().cloned()
}

pub fn install(routes: &mut Registry, ctx: &mut HubCtx) {
    ctx.ext.insert(Arc::new(Windows::default()));
    routes.route(Verb::Post, Pattern::Exact("/popout"), popout);
}

/// The `close_app` tool (tools.mjs): ends every window of `name`, pinned
/// or not; its answer text.
pub fn close_app(ctx: &HubCtx, name: &str) -> String {
    let mut n = 0;
    if let Some(w) = windows(ctx) {
        for (k, win) in w.table().iter_mut() {
            if k == name || k.starts_with(&format!("{name}@")) {
                win.kill();
                n += 1;
            }
        }
    }
    if n > 0 {
        format!("closed {n} window(s).")
    } else {
        format!("no open window for {name}.")
    }
}

/// `/app/version` after the head moved to `v`: an open (unpinned) window
/// of the app that shows another version closes, and a press of
/// `/popout` reopens it at the hints it had.
pub fn reopen(ctx: &HubCtx, name: Option<&Value>, v: f64) {
    let Some(w) = windows(ctx) else { return };
    let key = name.map(js::string_of).unwrap_or_else(|| "undefined".into());
    let hints = {
        let mut table = w.table();
        match table.get_mut(&key) {
            Some(win) if win.open() && win.shown_v != v => {
                win.kill();
                let hints = (win.hint_w, win.hint_h);
                table.remove(&key);
                hints
            }
            _ => return,
        }
    };
    let mut body = serde_json::Map::new();
    if let Some(n) = name {
        body.insert("name".into(), n.clone());
    }
    body.insert("w".into(), number(hints.0));
    body.insert("h".into(), number(hints.1));
    let body = Value::Object(body);
    tokio::spawn(async move {
        crate::hub::tools::artifact::self_post("/popout", &body).await;
    });
}

fn number(f: f64) -> Value {
    crate::hub::library::appmeta::num(f)
}

/// `${n}` of a number.
fn tpl(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else {
        js::number(f)
    }
}

/// `Math.max`/`Math.min`: NaN wins.
fn max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

fn min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

fn env_or(k: &str, default: &str) -> String {
    buwiz_protocol::env::var(k).filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_owned())
}

/// config.mjs: `NSH`, and `NSH_HOST`, else `nsh-macos-host` beside NSH
/// when NSH is a path, else on PATH.
fn nsh_bins() -> (String, String) {
    let nsh = env_or("NSH", "nsh");
    let host = std::env::var("NSH_HOST").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| {
        if nsh.contains('/') {
            Path::new(&nsh).parent().unwrap_or(Path::new("")).join("nsh-macos-host").to_string_lossy().into_owned()
        } else {
            "nsh-macos-host".into()
        }
    });
    (nsh, host)
}

/// `execFileSync(bin, args, {encoding: "utf8"})`: stdout, or `None` where
/// Node threw (no binary, a non-zero exit).
async fn exec(bin: &str, args: &[&str]) -> Option<String> {
    let out = tokio::process::Command::new(bin).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().await.ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `desktopBounds()`: the Finder desktop's width and height, asked once
/// per process; 1440×900 when that fails.
async fn desktop_bounds() -> (f64, f64) {
    static BOUNDS: OnceLock<(f64, f64)> = OnceLock::new();
    if let Some(b) = BOUNDS.get() {
        return *b;
    }
    let got = exec("osascript", &["-e", r#"tell application "Finder" to get bounds of window of desktop"#]).await.map(|s| {
        let n: Vec<f64> = s.trim().split(',').map(js_number_str).collect();
        let or = |v: Option<&f64>, d: f64| v.copied().filter(|f| *f != 0.0 && !f.is_nan()).unwrap_or(d);
        (or(n.get(2), 1440.0), or(n.get(3), 900.0))
    });
    *BOUNDS.get_or_init(|| got.unwrap_or((1440.0, 900.0)))
}

const LIST: [&str; 4] = ["list-windows", "--all", "--format", "%{window-id} %{window-title}"];

/// The AeroSpace id of the first window whose title contains `title`.
async fn window_id(title: &str) -> Option<String> {
    let out = exec("aerospace", &LIST).await?;
    let line = out.split('\n').find(|l| l.contains(title))?;
    line.trim().split_whitespace().next().map(str::to_owned)
}

/// `raiseWidget(title)`: pull an open pop-out onto the focused workspace
/// and focus it. AeroSpace-only, every failure ignored. (A title
/// `widget · R1` also finds `widget · R12`, as in Node.)
async fn raise(title: &str) {
    let Some(wid) = window_id(title).await else { return };
    let Some(ws) = exec("aerospace", &["list-workspaces", "--focused"]).await else { return };
    if exec("aerospace", &["move-node-to-workspace", "--window-id", &wid, "--focus-follows-window", ws.trim()]).await.is_some() {
        exec("aerospace", &["focus", "--window-id", &wid]).await;
    }
}

/// AeroSpace tiles every new window: float ours once its title lands, on
/// the focused workspace, focused. Every 600 ms, 40 tries at most, until
/// the window exits.
fn float(name: String, exited: Arc<AtomicBool>) {
    tokio::spawn(async move {
        let title = format!("widget · {name}");
        let mut tries = 0;
        loop {
            tokio::time::sleep(Duration::from_millis(600)).await;
            tries += 1;
            if tries > 40 || exited.load(Ordering::SeqCst) {
                return;
            }
            let Some(wid) = window_id(&title).await else { continue };
            let Some(ws) = exec("aerospace", &["list-workspaces", "--focused"]).await else { continue };
            if exec("aerospace", &["layout", "floating", "--window-id", &wid]).await.is_none() {
                continue;
            }
            if exec("aerospace", &["move-node-to-workspace", "--window-id", &wid, "--focus-follows-window", ws.trim()]).await.is_none() {
                continue;
            }
            if exec("aerospace", &["focus", "--window-id", &wid]).await.is_none() {
                continue;
            }
            println!("  /popout {name} → window {wid}");
            return;
        }
    });
}

/// A JavaScript template's text with its `${…}`s filled from `vars`, in
/// one pass (a value is never read for placeholders).
pub(crate) fn fill(template: &str, vars: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len() + 256);
    let mut rest = template;
    while let Some(at) = rest.find("${") {
        let end = rest[at..].find('}').map(|e| at + e).unwrap_or(rest.len());
        let key = &rest[at + 2..end];
        out.push_str(&rest[..at]);
        match vars.iter().find(|(k, _)| *k == key) {
            Some((_, v)) => out.push_str(v),
            None => out.push_str(&rest[at..=end.min(rest.len() - 1)]),
        }
        rest = &rest[(end + 1).min(rest.len())..];
    }
    out.push_str(rest);
    out
}

fn reply(v: &Value) -> HubResponse {
    answer(200, out(v), true)
}

/// `POST /popout {name, v?, w?, h?}`.
async fn popout(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let Some(wins) = windows(&ctx) else { return reply(&json!({"ok": false})) };
    let _one = wins.serial.lock().await;
    // a body that isn't JSON, or is null, leaves every field at its default
    let j = parse(&req.body).ok().filter(|j| !j.is_null()).unwrap_or(json!({}));
    let field = |k: &str| j.as_object().and_then(|o| o.get(k));
    let name = field("name").filter(|n| js::truthy(n)).map(js::string_of).unwrap_or_default();
    let pop_v = field("v").filter(|v| js::truthy(v)).map(|v| js_number(Some(v))).filter(|v| *v != 0.0 && !v.is_nan());
    let hint = |k: &str| Some(js_number(field(k))).filter(|f| *f > 0.0).unwrap_or(0.0);
    let (hint_w, hint_h) = (hint("w"), hint("h"));
    if !(name.len() > 1 && name.starts_with('R') && name[1..].bytes().all(|b| b.is_ascii_digit())) {
        return reply(&json!({"ok": false}));
    }
    let suffix = |sep: &str| pop_v.map(|v| format!("{sep}{}", tpl(v))).unwrap_or_default();
    let key = format!("{name}{}", suffix("@v"));
    // the version the window shows: the pinned one, else the head
    let shown_v = pop_v.unwrap_or_else(|| head_of(&ctx, &name).ok().flatten().map(|v| v as f64).unwrap_or(0.0));
    // already open: raise it rather than rewrite the wrapper under it (a
    // rewrite remounts the module); a window on an older head is replaced
    let raise_it = {
        let mut table = wins.table();
        match table.get_mut(&key) {
            Some(prev) if prev.open() => {
                if prev.shown_v == shown_v {
                    true
                } else {
                    prev.kill();
                    table.remove(&key);
                    false
                }
            }
            _ => false,
        }
    };
    if raise_it {
        raise(&format!("widget · {name}")).await;
        return reply(&json!({"ok": true, "already": true}));
    }
    let dirs = Dirs::of(&ctx.data);
    let ver_path = pop_v.and_then(|v| ver_file(&dirs, &name, &tpl(v)));
    let widget_file = dirs.widgets.join(format!("{name}.tsx"));
    let gen_file = dirs.gen.join(format!("{name}.tsx"));
    let file = match (&ver_path, pop_v) {
        (Some(p), _) if p.exists() => Some(p.clone()),
        (_, Some(_)) => None,
        _ if widget_file.exists() => Some(widget_file.clone()),
        _ if gen_file.exists() => Some(gen_file.clone()),
        _ => None,
    };
    let Some(file) = file else { return reply(&json!({"ok": false, "error": "no such module"})) };
    let rel = if ver_path.as_ref() == Some(&file) {
        let base = file.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        format!("../generated/{name}/{}", base.strip_suffix(".tsx").unwrap_or(&base))
    } else if file == widget_file {
        format!("../widgets/{name}")
    } else {
        format!("../generated/{name}")
    };
    // the window hugs content height up to three quarters of the screen
    let cap_h = max(560.0, (desktop_bounds().await.1 * 0.75).floor());
    // the module's declared size, so the window opens at it
    let src = std::fs::read(&file).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    static SIZE: OnceLock<regex_lite::Regex> = OnceLock::new();
    static WIDTH: OnceLock<regex_lite::Regex> = OnceLock::new();
    let size_m = SIZE
        .get_or_init(|| regex_lite::Regex::new(r"export\s+const\s+popoutSize\s*=\s*\{[^}]*?w\s*:\s*(\d+(?:\.\d+)?)[^}]*?h\s*:\s*(\d+(?:\.\d+)?)").expect("the size pattern"))
        .captures(&src)
        .map(|c| (js_number_str(&c[1]), js_number_str(&c[2])));
    let width_m = WIDTH.get_or_init(|| regex_lite::Regex::new(r"export\s+const\s+popoutWidth\s*=\s*(\d+(?:\.\d+)?)").expect("the width pattern")).captures(&src).map(|c| js_number_str(&c[1]));
    let pop_w = env_or("POP_W", "360");
    let pop_h = env_or("POP_H", "480");
    let w_in = size_m.map(|s| s.0).or(width_m).unwrap_or_else(|| if hint_w != 0.0 { hint_w } else { js_number_str(&pop_w) });
    let want_w = min(720.0, max(240.0, js_round(w_in)));
    let want_h = match size_m {
        Some((_, h)) => min(cap_h, max(120.0, js_round(h))),
        None if hint_h != 0.0 => min(cap_h, max(120.0, js_round(hint_h))),
        None => js_number_str(&pop_h),
    };
    let pop_name = match pop_v {
        Some(v) => format!("{name}@{}", tpl(v)),
        None => name.clone(),
    };
    let app_title = ctx
        .db
        .with(|c| c.query_row("SELECT title FROM apps WHERE name=?", [&name], |r| r.get::<_, Option<String>>(0)).optional())
        .ok()
        .flatten()
        .flatten()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| name.clone());
    let ver_label = if shown_v != 0.0 {
        let v = if shown_v.fract() == 0.0 { rusqlite::types::Value::Integer(shown_v as i64) } else { rusqlite::types::Value::Real(shown_v) };
        ctx.db
            .with(|c| c.query_row("SELECT label FROM versions WHERE name=? AND v=?", rusqlite::params![name, v], |r| r.get::<_, Option<String>>(0)).optional())
            .ok()
            .flatten()
            .flatten()
            .unwrap_or_default()
    } else {
        String::new()
    };
    let badge = if shown_v != 0.0 { format!("v{}{}", tpl(shown_v), if ver_label.is_empty() { String::new() } else { format!(" · {ver_label}") }) } else { String::new() };
    let win_dir = ctx.data.join("windows");
    let _ = std::fs::create_dir_all(&win_dir);
    let wrapper = fill(
        WRAPPER,
        &[
            ("rel", rel),
            ("POP_W", pop_w.clone()),
            ("hintW || POP_W", if hint_w != 0.0 { tpl(hint_w) } else { pop_w.clone() }),
            ("capH", tpl(cap_h)),
            ("JSON.stringify(popName)", js::stringify(&Value::String(pop_name))),
            ("wantH", tpl(want_h)),
            ("hintH ? wantH : 0", if hint_h != 0.0 { tpl(want_h) } else { "0".into() }),
            ("JSON.stringify(badge)", js::stringify(&Value::String(badge))),
        ],
    );
    let wrapper_file = win_dir.join(format!("pop-{name}{}.tsx", suffix("-")));
    let _ = std::fs::write(&wrapper_file, &wrapper);
    // compile the wrapper once (cached by content), then a bare host on
    // the bundle
    let build_key: String = Sha1::digest(wrapper.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>()[..10].to_owned();
    let build_dir = win_dir.join(format!(".pop-{name}{}-{build_key}", suffix("-")));
    let bundle = build_dir.join("bundle.nshc");
    let (nsh, host) = nsh_bins();
    if !bundle.exists() {
        let _ = std::fs::create_dir_all(&build_dir);
        let b = tokio::process::Command::new(&nsh).arg("build").arg(&wrapper_file).arg("-o").arg(&build_dir).stdin(Stdio::null()).output().await;
        let failed = match &b {
            Ok(o) => !o.status.success() || !bundle.exists(),
            Err(_) => true,
        };
        if failed {
            let text = b.map(|o| {
                let err = String::from_utf8_lossy(&o.stderr).into_owned();
                if err.is_empty() {
                    String::from_utf8_lossy(&o.stdout).into_owned()
                } else {
                    err
                }
            });
            let text = text.unwrap_or_default();
            let last = text.trim().rsplit('\n').next().unwrap_or("").to_owned();
            return reply(&json!({"ok": false, "error": format!("build failed: {last}")}));
        }
    }
    let shown_name = if shown_v != 0.0 { format!("{app_title} · v{}", tpl(shown_v)) } else { app_title };
    let bundle_id = env_or("BUWIZ_BUNDLE_ID", "dev.goldcoders.grokshell");
    let size = format!("{}x{}", tpl(want_w), tpl(want_h + 26.0));
    let spawned = tokio::process::Command::new(&host)
        .arg("--bundle")
        .arg(&bundle)
        .args(["--name", &shown_name, "--app-id", &format!("{bundle_id}.widget"), "--window-size", &size])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    // Node's spawn emits an unhandled 'error' here and the broker dies
    // after answering {ok: true}; this answers the failure instead (D8)
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => return reply(&json!({"ok": false, "error": format!("spawn {host}: {e}")})),
    };
    let Some(pid) = child.id() else { return reply(&json!({"ok": false, "error": format!("spawn {host}: no pid")})) };
    let id = wins.next.fetch_add(1, Ordering::SeqCst);
    let exited = Arc::new(AtomicBool::new(false));
    wins.table().insert(key.clone(), Win { id, pid, shown_v, hint_w, hint_h, killed: false, exited: exited.clone() });
    // a replaced window exits after its successor took the key
    {
        let (wins, exited, key) = (wins.clone(), exited.clone(), key);
        tokio::spawn(async move {
            let _ = child.wait().await;
            exited.store(true, Ordering::SeqCst);
            let mut table = wins.table();
            if table.get(&key).is_some_and(|w| w.id == id) {
                table.remove(&key);
            }
        });
    }
    println!("  /popout {name} → pid {pid}");
    float(name, exited);
    reply(&json!({"ok": true, "pid": pid}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popout_wrapper_fills_every_placeholder_once() {
        let vars: Vec<(&str, String)> =
            ["rel", "POP_W", "hintW || POP_W", "capH", "JSON.stringify(popName)", "wantH", "hintH ? wantH : 0", "JSON.stringify(badge)"].iter().map(|k| (*k, format!("<{k}>"))).collect();
        let s = fill(WRAPPER, &vars);
        assert!(!s.contains("${"), "every placeholder is known");
        assert!(s.contains("import R, * as mod from \"<rel>\";"));
        assert!(s.contains("    {<JSON.stringify(badge)> ? (\n"));
        // a value is never filled again
        let s = fill(WRAPPER, &[("rel", "${capH}".into())]);
        assert!(s.contains("from \"${capH}\";"));
        assert!(s.starts_with("import { mount, Text, View } from \"nsh\";\n") && s.ends_with("));\n"));
    }

    #[test]
    fn popout_numbers_read_as_javascript_writes_them() {
        assert_eq!((tpl(512.4), tpl(f64::NAN), tpl(675.0)), ("512.4".into(), "NaN".into(), "675".into()));
        assert!(max(240.0, f64::NAN).is_nan() && min(720.0, f64::NAN).is_nan());
        assert_eq!(min(720.0, max(240.0, js_round(512.4))), 512.0);
    }
}
