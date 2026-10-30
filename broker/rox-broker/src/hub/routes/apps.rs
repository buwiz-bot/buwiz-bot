//! The mini-app routes (5.7b; routes/apps.mjs without the Rust-owned
//! routes, `/models` and `/popout`): `/module`, `/app/*`, `/apps/meta`,
//! `/artifacts`, `/widgets`, `/publish`, `/unpublish`, `/reuse`,
//! `/catalog`, `/widget-meta` and `/sources`; and at the hub's start what
//! app-server.mjs runs after the store's start: the pre-versioning
//! migration, the pre-slug renames, the index, and the chunk backfill.
//!
//! `/app/version` does not reopen an open pop-out on the new head: the
//! pop-outs are Node's office role's (`/popout`, D36).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::types::Value as SqlValue;
use rusqlite::OptionalExtension;
use serde_json::{json, Map, Value};

use super::chat::{answer, origin_of, parse};
use crate::hub::ctx::HubCtx;
use crate::hub::library::appmeta::{self, app_info, apps_meta, gen_app_icon, gen_dir, icon_path, icon_url, image_capable, is_r_name, mtime_ms, num, rows_of, trim_js, update_app_meta};
use crate::hub::library::artifacts::{self, forget_artifact, mcp_app_module};
use crate::hub::pipeline::chunks::emit_chunk;
use crate::hub::pipeline::versions::{head_of, read_dir_sorted, set_head, ver_file, ver_slug, versions_of, walk_files};
use crate::hub::pipeline::Dirs;
use crate::hub::store::json::{js_number, js_slice, out, template};
use crate::hub::store::{uuidv7, Turn};
use crate::hub::types::{HubRequest, HubResponse, Pattern::Exact as X, Pattern::Prefix as P, Verb};
use crate::hub::Registry;
use crate::js;

// ————— answers —————

pub(crate) fn json_at(status: u16, v: &Value, ct: bool) -> HubResponse {
    answer(status, out(v), ct)
}

/// `{ok: false, error}` with `status`.
pub(crate) fn fail(status: u16, e: &str, ct: bool) -> HubResponse {
    json_at(status, &json!({"ok": false, "error": e}), ct)
}

/// Node's dispatcher answering a throw: 500 `{error}`.
fn crashed(e: &str, ct: bool) -> HubResponse {
    json_at(500, &json!({"error": e}), ct)
}

/// A file served as is (`image/png`, no-cache), or 404 "no icon".
pub(crate) fn icon(p: Option<&Path>) -> HubResponse {
    match p.and_then(|p| std::fs::read(p).ok()) {
        Some(bytes) => {
            use http_body_util::{BodyExt, Full};
            let mut res = hyper::Response::new(Full::new(bytes::Bytes::from(bytes)).map_err(|never| match never {}).boxed());
            res.headers_mut().insert(hyper::header::CONTENT_TYPE, hyper::header::HeaderValue::from_static("image/png"));
            res.headers_mut().insert(hyper::header::CACHE_CONTROL, hyper::header::HeaderValue::from_static("no-cache"));
            res
        }
        None => answer(404, "no icon".into(), false),
    }
}

/// `const { k, … } = JSON.parse(body || "{}")`: the body, or why it threw.
pub(crate) fn body(req: &HubRequest) -> Result<Value, String> {
    let v = parse(&req.body)?;
    if v.is_null() {
        return Err("Cannot destructure property 'name' of 'JSON.parse(...)' as it is null.".into());
    }
    Ok(v)
}

pub(crate) fn get<'a>(b: &'a Value, k: &str) -> Option<&'a Value> {
    b.as_object().and_then(|o| o.get(k))
}

/// `String(v || "")`.
pub fn str_or(v: Option<&Value>) -> String {
    match v {
        Some(v) if js::truthy(v) => js::string_of(v),
        _ => String::new(),
    }
}

/// A value as node:sqlite binds it; what it refuses (undefined, objects,
/// booleans) is its own error.
fn bind(v: Option<&Value>) -> Result<SqlValue, String> {
    match v {
        Some(Value::Null) => Ok(SqlValue::Null),
        Some(Value::String(s)) => Ok(SqlValue::Text(s.clone())),
        Some(Value::Number(n)) => Ok(n.as_i64().map(SqlValue::Integer).unwrap_or_else(|| SqlValue::Real(n.as_f64().unwrap_or(f64::NAN)))),
        _ => Err("Provided value cannot be bound to SQLite parameter 1.".into()),
    }
}

/// `Number(v)` as node:sqlite binds it (NaN as NULL).
fn bind_number(f: f64) -> SqlValue {
    if !f.is_finite() {
        SqlValue::Null
    } else if f.fract() == 0.0 {
        SqlValue::Integer(f as i64)
    } else {
        SqlValue::Real(f)
    }
}

fn widget_row(ctx: &HubCtx, name: &SqlValue) -> Result<bool, String> {
    ctx.db.with(|c| c.query_row("SELECT 1 FROM widgets WHERE name=?", [name], |_| Ok(())).optional().map(|r| r.is_some()))
}

/// Runs `f` after `ms` (Node's `setTimeout`).
fn later(ms: u64, f: impl FnOnce() + Send + 'static) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        f();
    });
}

// ————— install and the start —————

pub fn install(routes: &mut Registry, ctx: &mut HubCtx) {
    artifacts::seams(ctx);
    migrate(ctx);
    routes.on_start(|ctx| {
        tokio::spawn(backfill(ctx));
    });
    routes.route(Verb::Get, P("/module"), module);
    routes.route(Verb::Post, X("/app/meta"), app_meta);
    routes.route(Verb::Get, X("/app/info"), app_info_route);
    routes.route(Verb::Get, X("/apps/meta"), apps_meta_route);
    routes.route(Verb::Get, X("/app/icon"), app_icon);
    routes.route(Verb::Post, X("/app/icon/gen"), app_icon_gen);
    routes.route(Verb::Post, X("/app/version"), app_version);
    routes.route(Verb::Post, X("/app/version/label"), app_version_label);
    routes.route(Verb::Get, X("/artifacts"), artifacts_route);
    routes.route(Verb::Get, X("/widgets"), widgets);
    routes.route(Verb::Post, X("/publish"), publish);
    routes.route(Verb::Post, X("/unpublish"), unpublish);
    routes.route(Verb::Post, X("/app/delete"), app_delete);
    routes.route(Verb::Post, X("/reuse"), reuse);
    routes.route(Verb::Get, X("/sources"), sources);
    routes.route(Verb::Get, X("/catalog"), catalog);
    routes.route(Verb::Post, X("/widget-meta"), widget_meta);
    super::artifacts::install(routes);
}

/// `^(R\d+)\.tsx$` of a file name.
fn module_file(f: &str) -> Option<&str> {
    f.strip_suffix(".tsx").filter(|n| is_r_name(n))
}

/// The pre-versioning migration (the file as it stands becomes v1, the
/// head opens on it), the pre-slug renames (`v<n>.tsx` → `<slug>.v<n>.tsx`)
/// and the index after them.
pub fn migrate(ctx: &HubCtx) {
    let gen = gen_dir(ctx);
    let dirs = Dirs::of(&ctx.data);
    for e in read_dir_sorted(&gen).unwrap_or_default() {
        let f = e.file_name().to_string_lossy().into_owned();
        let Some(name) = module_file(&f) else { continue };
        if head_of(ctx, name).ok().flatten().is_some_and(|v| v != 0) {
            continue;
        }
        let _ = (|| -> Result<(), String> {
            let dir = gen.join(name);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let v1 = dir.join(format!("{}.v1.tsx", ver_slug(ctx, name)?));
            if ver_file(&dirs, name, "1").is_none() {
                std::fs::copy(gen.join(&f), &v1).map_err(|e| e.to_string())?;
            }
            let at = mtime_ms(&gen.join(&f)).ok_or("no file")?;
            ctx.db.with(|c| {
                c.execute("INSERT OR IGNORE INTO versions(name,v,parent,prompt,created_at) VALUES(?,1,NULL,NULL,?)", rusqlite::params![name, at])?;
                c.execute("INSERT OR IGNORE INTO heads(name,v) VALUES(?,1)", [name])
            })?;
            Ok(())
        })();
    }
    for e in read_dir_sorted(&gen).unwrap_or_default() {
        let d = e.file_name().to_string_lossy().into_owned();
        if !is_r_name(&d) {
            continue;
        }
        let Ok(list) = read_dir_sorted(&gen.join(&d)) else { continue };
        for x in list {
            let x = x.file_name().to_string_lossy().into_owned();
            let Some(n) = x.strip_prefix('v').and_then(|r| r.strip_suffix(".tsx")).filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) else { continue };
            let Ok(slug) = ver_slug(ctx, &d) else { break };
            if std::fs::rename(gen.join(&d).join(&x), gen.join(&d).join(format!("{slug}.v{n}.tsx"))).is_err() {
                break;
            }
        }
    }
    ctx.store.write_index();
}

/// The chunk backfill: every module, published widget and version with no
/// chunk gets one, one build at a time.
async fn backfill(ctx: Arc<HubCtx>) {
    let Some(nsh) = ctx.nsh.clone() else { return };
    let dirs = Dirs::of(&ctx.data);
    for (dir, w) in [(&dirs.gen, ""), (&dirs.widgets, "w")] {
        if !dir.exists() {
            continue;
        }
        for e in read_dir_sorted(dir).unwrap_or_default() {
            let f = e.file_name().to_string_lossy().into_owned();
            let Some(name) = module_file(&f) else { continue };
            let key = format!("{name}{w}");
            if !dirs.chunks.join(format!("{key}.js")).exists() {
                emit_chunk(nsh.clone(), dirs.stage.clone(), dirs.chunks.clone(), key, dir.join(&f)).await;
            }
        }
    }
    for e in read_dir_sorted(&dirs.gen).unwrap_or_default() {
        let d = e.file_name().to_string_lossy().into_owned();
        if !is_r_name(&d) {
            continue;
        }
        for vf in read_dir_sorted(&dirs.gen.join(&d)).unwrap_or_default() {
            let vf = vf.file_name().to_string_lossy().into_owned();
            let Some(v) = appmeta::version_of_file(&vf) else { continue };
            let key = format!("{d}v{v}");
            if !dirs.chunks.join(format!("{key}.js")).exists() {
                emit_chunk(nsh.clone(), dirs.stage.clone(), dirs.chunks.clone(), key, dirs.gen.join(&d).join(&vf)).await;
            }
        }
    }
}

// ————— the routes —————

/// `GET /module?name=R<n>[@v<k>]` (or `mcp:<server>/<app>[|<props>]`).
async fn module(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let raw = req.query("name").unwrap_or_default();
    if raw.starts_with("mcp:") {
        return match mcp_app_module(&ctx, &raw).await {
            Ok(v) => json_at(200, &v, true),
            Err(e) => crashed(&e, true),
        };
    }
    let (name, ver) = match raw.split_once("@v") {
        Some((n, v)) if is_r_name(n) && !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) => (n.to_owned(), Some(js_number(Some(&json!(v))))),
        None if is_r_name(&raw) => (raw.clone(), None),
        _ => (String::new(), None),
    };
    let dirs = Dirs::of(&ctx.data);
    let file = match ver.filter(|v| *v != 0.0) {
        Some(v) => ver_file(&dirs, &name, &js::number(v)),
        None => [&dirs.gen, &dirs.widgets].iter().map(|d| d.join(format!("{name}.tsx"))).find(|p| p.exists()),
    };
    let published = match widget_row(&ctx, &SqlValue::Text(name.clone())) {
        Ok(p) => p,
        Err(e) => return crashed(&e, true),
    };
    let source = file.filter(|_| !name.is_empty()).and_then(|f| std::fs::read(f).ok());
    let Some(source) = source else { return json_at(200, &json!({"source": "", "published": published}), true) };
    let head = head_of(&ctx, &name).ok().flatten().map_or(Value::Null, |v| json!(v));
    let title = ctx
        .db
        .with(|c| c.query_row("SELECT title FROM apps WHERE name=?", [&name], |r| r.get::<_, SqlValue>(0)).optional())
        .ok()
        .flatten()
        .map(crate::hub::store::write::to_json)
        .filter(js::truthy)
        .unwrap_or(Value::Null);
    let v = match ver {
        Some(v) => num(v),
        None => head.clone(),
    };
    json_at(
        200,
        &json!({
            "source": String::from_utf8_lossy(&source),
            "published": published,
            "v": v,
            "head": head,
            "versions": versions_of(&ctx, &name).unwrap_or_default(),
            "title": title,
        }),
        true,
    )
}

/// `POST /app/meta {name, title?, file?, description?, w?, h?, icon?}`:
/// answers /app/info's object.
async fn app_meta(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let r = async {
        let j = body(&req)?;
        let name = str_or(get(&j, "name"));
        update_app_meta(&ctx, &name, &j).await?;
        app_info(&ctx, &name, &origin_of(&req))
    }
    .await;
    match r {
        Ok(v) => json_at(200, &v, true),
        Err(e) => fail(400, &e, true),
    }
}

async fn app_info_route(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let name = req.query("name").unwrap_or_default();
    match app_info(&ctx, &name, &origin_of(&req)) {
        Ok(v) => json_at(200, &v, true),
        Err(e) => fail(404, &e, true),
    }
}

async fn apps_meta_route(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    match apps_meta(&ctx, &origin_of(&req)) {
        Ok(apps) => json_at(200, &json!({"apps": apps}), true),
        Err(e) => crashed(&e, true),
    }
}

async fn app_icon(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let name = req.query("name").unwrap_or_default();
    let p = is_r_name(&name).then(|| icon_path(&ctx, &name)).filter(|p| p.exists());
    icon(p.as_deref())
}

/// `POST /app/icon/gen {name, prompt?, model}`: starts the drawing and
/// answers /app/info at once (`generating`).
async fn app_icon_gen(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let r = async {
        let j = body(&req)?;
        let n = str_or(get(&j, "name"));
        let model = str_or(get(&j, "model"));
        if !image_capable(&model).await {
            return Err(format!("{} can't generate images", if model.is_empty() { "This model" } else { &model }));
        }
        let run = gen_app_icon(ctx.clone(), &n, &str_or(get(&j, "prompt")), &model);
        tokio::spawn(async move {
            let _ = run.await;
        });
        app_info(&ctx, &n, &origin_of(&req))
    }
    .await;
    match r {
        Ok(v) => json_at(200, &v, true),
        Err(e) => fail(400, &e, true),
    }
}

/// `POST /app/version {name, v}`: the head moves to v.
async fn app_version(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let r = (|| -> Result<_, String> {
        let j = body(&req)?;
        let name = str_or(get(&j, "name"));
        let v = js_number(get(&j, "v"));
        let moved = if v.is_finite() && v.fract() == 0.0 { set_head(&ctx, &name, v as i64)?.is_some() } else { false };
        if !moved {
            return Err("no such version".to_owned());
        }
        Ok(json!({"ok": true, "v": num(v)}))
    })();
    match r {
        Ok(v) => json_at(200, &v, true),
        Err(e) => fail(400, &e, true),
    }
}

/// `POST /app/version/label {name, v, label}`: names a version ("" clears).
async fn app_version_label(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let r = (|| -> Result<_, String> {
        let j = body(&req)?;
        let label = js_slice(trim_js(&str_or(get(&j, "label"))), 40);
        let label = if label.is_empty() { SqlValue::Null } else { SqlValue::Text(label) };
        let name = str_or(get(&j, "name"));
        let v = bind_number(js_number(get(&j, "v")));
        let n = ctx.db.with(|c| c.execute("UPDATE versions SET label=? WHERE name=? AND v=?", rusqlite::params![label, name, v]))?;
        if n == 0 {
            return Err("no such version".to_owned());
        }
        Ok(())
    })();
    match r {
        Ok(()) => json_at(200, &json!({"ok": true}), true),
        Err(e) => fail(400, &e, true),
    }
}

/// `String.prototype.localeCompare` as ICU's root collation orders plain
/// titles: case and accents only break ties; spaces and punctuation before
/// digits, digits before letters.
pub fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |s: &str| -> Vec<(u8, String)> {
        s.chars()
            .map(|c| {
                let lower: String = c.to_lowercase().collect();
                let class = if c.is_alphabetic() {
                    2
                } else if c.is_numeric() {
                    1
                } else {
                    0
                };
                (class, lower)
            })
            .collect()
    };
    key(a).cmp(&key(b)).then_with(|| {
        // a tie on letters: lowercase first, as ICU's tertiary level
        for (x, y) in a.chars().zip(b.chars()) {
            if x != y {
                return match (x.is_lowercase(), y.is_lowercase()) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => x.cmp(&y),
                };
            }
        }
        a.len().cmp(&b.len())
    })
}

/// `GET /artifacts?q=&thread=`: the composer's @ picker: the thread's apps
/// first, then every app and file, by title; 40 at most.
async fn artifacts_route(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let q = req.query("q").unwrap_or_default().to_lowercase();
    let tid = req.query("thread").unwrap_or_default();
    let scoped: HashSet<String> = ctx.store.read(|s| {
        s.list
            .iter()
            .find(|t| !tid.is_empty() && t.id == tid)
            .map_or_else(HashSet::new, |t| t.turns.iter().filter(|u| u.role == "ai").filter_map(|u| u.name.clone().filter(|n| is_r_name(n))).collect())
    });
    let lookups = ctx.db.with(|c| {
        let names = |sql: &str| -> rusqlite::Result<HashSet<String>> { Ok(rows_of(c, sql, &[])?.iter().map(|r| r.get("name").map(js::string_of).unwrap_or_default()).collect()) };
        let metas: HashMap<String, Map<String, Value>> =
            rows_of(c, "SELECT name,title,file,description FROM apps", &[])?.into_iter().map(|r| (r.get("name").map(js::string_of).unwrap_or_default(), r)).collect();
        Ok((names("SELECT name FROM tombstones")?, names("SELECT name FROM widgets")?, metas))
    });
    let (tomb, pubs, metas) = match lookups {
        Ok(l) => l,
        Err(e) => return crashed(&e, true),
    };
    let hints = appmeta::hints(&ctx);
    let origin = origin_of(&req);
    let gen = gen_dir(&ctx);
    let mut items: Vec<Value> = Vec::new();
    let list = match read_dir_sorted(&gen) {
        Ok(l) => l,
        Err(e) => return crashed(&e.to_string(), true),
    };
    let empty = Map::new();
    for e in list {
        let f = e.file_name().to_string_lossy().into_owned();
        let Some(name) = module_file(&f) else { continue };
        if tomb.contains(name) {
            continue;
        }
        let meta = metas.get(name).unwrap_or(&empty);
        let title = meta.get("title").filter(|t| js::truthy(t)).map(js::string_of).unwrap_or_else(|| name.to_owned());
        if !q.is_empty() && !name.to_lowercase().contains(&q) && !title.to_lowercase().contains(&q) {
            continue;
        }
        let description = match meta.get("description") {
            Some(d) if !d.is_null() => d.clone(),
            _ => hints.get(name).map(|h| h.desc.clone()).filter(|d| !d.is_null()).unwrap_or(json!("")),
        };
        items.push(json!({
            "kind": "app",
            "name": name,
            "title": title,
            "file": meta.get("file").filter(|v| js::truthy(v)).cloned().unwrap_or_else(|| json!(f)),
            "description": description,
            "icon": icon_url(&ctx, name, &origin),
            "v": head_of(&ctx, name).ok().flatten().map_or(Value::Null, |v| json!(v)),
            "published": pubs.contains(name),
            "uri": format!("module://{name}"),
            "scope": if scoped.contains(name) { "thread" } else { "global" },
        }));
    }
    let out_dir = gen.join("files");
    for f in walk_files(&out_dir) {
        let name = f.get("name").map(js::string_of).unwrap_or_default();
        if !q.is_empty() && !name.to_lowercase().contains(&q) {
            continue;
        }
        let rel = f.get("rel").map(js::string_of).unwrap_or_default();
        items.push(json!({"kind": "file", "name": name, "title": name, "file": rel, "uri": format!("file://{}", out_dir.join(&rel).display()), "scope": "global"}));
    }
    items.sort_by(|a, b| {
        let rank = |x: &Value| if x["scope"] == "thread" { 0 } else { 1 };
        rank(a).cmp(&rank(b)).then_with(|| locale_compare(&js::string_of(&a["title"]), &js::string_of(&b["title"])))
    });
    items.truncate(40);
    json_at(200, &json!({"items": items}), true)
}

async fn widgets(ctx: Arc<HubCtx>, _: HubRequest) -> HubResponse {
    match ctx.db.with(|c| rows_of(c, "SELECT name,title,prompt,published_at FROM widgets ORDER BY published_at", &[])) {
        Ok(rows) => json_at(200, &Value::Array(rows.into_iter().map(Value::Object).collect()), true),
        Err(e) => crashed(&e, true),
    }
}

/// `/error|implicit|rejected/i`.
fn failed_caption(s: &str) -> bool {
    let l = s.to_lowercase();
    ["error", "implicit", "rejected"].iter().any(|w| l.contains(w))
}

/// `POST /publish {name}`: the app copied to widgets/ with its caption.
async fn publish(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let r = (|| -> Result<_, String> {
        let j = body(&req)?;
        let name = template(get(&j, "name"));
        let src = gen_dir(&ctx).join(format!("{name}.tsx"));
        if !is_r_name(&name) || !src.exists() {
            return Err(format!("no such generated module {name}"));
        }
        let (mut title, prompt) = ctx.store.read(|s| {
            for t in &s.list {
                let Some(i) = t.turns.iter().position(|x| x.role == "ai" && x.name.as_deref() == Some(name.as_str())) else { continue };
                let title = t.turns[i].caption.clone().unwrap_or_default();
                let prompt = t.turns[..i].iter().rev().find(|u| u.role == "user").map(|u| u.text.clone().unwrap_or_default()).unwrap_or_default();
                return (title, prompt);
            }
            (String::new(), String::new())
        });
        if title.is_empty() || failed_caption(&title) {
            title = Some(js_slice(&prompt, 48)).filter(|p| !p.is_empty()).unwrap_or_else(|| name.clone());
        }
        let widgets = ctx.data.join("widgets");
        let dest = widgets.join(format!("{name}.tsx"));
        std::fs::copy(&src, &dest).map_err(|e| e.to_string())?;
        let at = js::iso(crate::hub::store::now_ms() as f64);
        ctx.db.with(|c| c.execute("INSERT OR REPLACE INTO widgets(name,title,prompt,published_at) VALUES(?,?,?,?)", rusqlite::params![name, title, prompt, at]))?;
        let dirs = Dirs::of(&ctx.data);
        if let Some(nsh) = ctx.nsh.clone() {
            tokio::spawn(emit_chunk(nsh, dirs.stage, dirs.chunks, format!("{name}w"), dest));
        }
        ctx.store.write_shelf()?;
        ctx.store.write_index();
        Ok(json!({"ok": true, "title": title}))
    })();
    match r {
        Ok(v) => json_at(200, &v, true),
        Err(e) => fail(400, &e, false),
    }
}

/// `POST /unpublish {name}`: the row at once, the file 700 ms later (after
/// the shelf and the index stop importing it).
async fn unpublish(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let r = (|| -> Result<_, String> {
        let j = body(&req)?;
        let name = get(&j, "name");
        let bound = bind(name)?;
        ctx.db.with(|c| c.execute("DELETE FROM widgets WHERE name=?", [&bound]))?;
        ctx.store.write_shelf()?;
        ctx.store.write_index();
        let file = ctx.data.join("widgets").join(format!("{}.tsx", template(name)));
        later(700, move || {
            let _ = std::fs::remove_file(file);
        });
        Ok(())
    })();
    match r {
        Ok(()) => json_at(200, &json!({"ok": true}), true),
        Err(e) => fail(400, &e, false),
    }
}

/// `POST /app/delete {name, everywhere?}`: rows, tombstone, every card
/// naming it and the prompt above it, and its files 700 ms later.
async fn app_delete(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let r = (|| -> Result<_, String> {
        let j = body(&req)?;
        let raw = get(&j, "name");
        let name = str_or(raw);
        if !is_r_name(&name) {
            return Err(format!("no such module {}", template(raw)));
        }
        let everywhere = get(&j, "everywhere").is_some_and(js::truthy);
        ctx.db.with(|c| {
            if everywhere {
                c.execute("DELETE FROM widgets WHERE name=?", [&name])?;
            }
            c.execute("DELETE FROM kv WHERE name=?", [&name])?;
            c.execute("DELETE FROM apps WHERE name=?", [&name])?;
            c.execute("INSERT OR IGNORE INTO tombstones(name) VALUES(?)", [&name])
        })?;
        forget_artifact(&ctx, &name)?;
        ctx.store.write(|s| {
            for t in s.list.iter_mut() {
                let mut drop = HashSet::new();
                for (i, u) in t.turns.iter().enumerate() {
                    if u.role == "ai" && u.name.as_deref() == Some(name.as_str()) {
                        drop.insert(i);
                        if i > 0 && t.turns[i - 1].role == "user" {
                            drop.insert(i - 1);
                        }
                    }
                }
                if !drop.is_empty() {
                    let mut i = 0;
                    t.turns.retain(|_| {
                        let keep = !drop.contains(&i);
                        i += 1;
                        keep
                    });
                }
            }
        })?;
        ctx.store.write_shelf()?;
        ctx.store.write_index();
        let dirs = Dirs::of(&ctx.data);
        let n = name.clone();
        later(700, move || {
            let _ = std::fs::remove_file(dirs.gen.join(format!("{n}.tsx")));
            let _ = std::fs::remove_dir_all(dirs.gen.join(&n));
            if everywhere {
                let _ = std::fs::remove_file(dirs.widgets.join(format!("{n}.tsx")));
            }
            let _ = (|| -> std::io::Result<()> {
                std::fs::remove_file(dirs.chunks.join(format!("{n}.js")))?;
                if everywhere {
                    std::fs::remove_file(dirs.chunks.join(format!("{n}w.js")))?;
                }
                for f in read_dir_sorted(&dirs.chunks)? {
                    let f = f.file_name().to_string_lossy().into_owned();
                    if f.starts_with(&format!("{n}v")) && f.ends_with(".js") {
                        std::fs::remove_file(dirs.chunks.join(&f))?;
                    }
                }
                Ok(())
            })();
        });
        ctx.db.with(|c| {
            c.execute("DELETE FROM versions WHERE name=?", [&name])?;
            c.execute("DELETE FROM heads WHERE name=?", [&name])
        })?;
        Ok(())
    })();
    match r {
        Ok(()) => json_at(200, &json!({"ok": true}), true),
        Err(e) => fail(400, &e, false),
    }
}

/// `POST /reuse {name}`: the published app as a widget card in the active
/// chat.
async fn reuse(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let r = (|| -> Result<_, String> {
        let j = body(&req)?;
        let raw = get(&j, "name");
        // BUG: no name reaches SQLite unchecked (Node's widgetRow(undefined));
        // the refusal carries node:sqlite's own message (ART-08)
        let bound = bind(raw)?;
        let name = template(raw);
        if !widget_row(&ctx, &bound)? || !ctx.data.join("widgets").join(format!("{name}.tsx")).exists() {
            return Err(format!("no published widget {name}"));
        }
        let turn = Turn {
            role: "ai".into(),
            id: Some(uuidv7()),
            name: Some(name),
            label: Some("widget".into()),
            caption: Some(String::new()),
            reaction: Some(String::new()),
            retries: Some(0),
            widget: true,
            ..Default::default()
        };
        ctx.store.write(|s| {
            let i = s.active_index();
            if let Some(t) = s.list.get_mut(i) {
                t.turns.push(turn);
            }
        })?;
        ctx.store.write_index();
        Ok(())
    })();
    match r {
        Ok(()) => json_at(200, &json!({"ok": true}), true),
        Err(e) => fail(400, &e, false),
    }
}

/// `GET /sources`: the data sources a generated module may call.
async fn sources(ctx: Arc<HubCtx>, _: HubRequest) -> HubResponse {
    let mut names = vec!["timeclock", "reminders"];
    if super::leftovers::linear_key(&ctx).await.is_some() {
        names.push("linear");
    }
    json_at(200, &json!({"sources": names.iter().map(|n| json!({"name": n})).collect::<Vec<_>>()}), true)
}

/// `GET /catalog`: every published widget as a Mini Apps card (kinds:
/// "db" saves state, "api" reads /data/, "keys" binds keys, else "mem").
async fn catalog(ctx: Arc<HubCtx>, _: HubRequest) -> HubResponse {
    let read = ctx.db.with(|c| {
        let meta = |like: &str| rows_of(c, &format!("SELECT k,v FROM meta WHERE k LIKE '{like}'"), &[]);
        Ok((
            meta("tags:%")?,
            meta("title:%")?,
            rows_of(c, "SELECT k FROM meta WHERE k LIKE 'arch:%'", &[])?,
            rows_of(c, "SELECT name,title,published_at FROM widgets ORDER BY published_at DESC", &[])?,
        ))
    });
    let (tag_rows, title_rows, arch_rows, list) = match read {
        Ok(r) => r,
        Err(e) => return crashed(&e, true),
    };
    let k = |r: &Map<String, Value>, f: &str| r.get(f).map(js::string_of).unwrap_or_default();
    let tags: HashMap<String, Vec<String>> =
        tag_rows.iter().map(|r| (k(r, "k").chars().skip(5).collect(), k(r, "v").split(',').map(|t| trim_js(t).to_owned()).filter(|t| !t.is_empty()).collect())).collect();
    let titles: HashMap<String, Value> = title_rows.iter().map(|r| (k(r, "k").chars().skip(6).collect(), r.get("v").cloned().unwrap_or(Value::Null))).collect();
    let arch: HashSet<String> = arch_rows.iter().map(|r| k(r, "k").chars().skip(5).collect()).collect();
    let widgets = ctx.data.join("widgets");
    let mut out = Vec::new();
    for w in list {
        let name = k(&w, "name");
        let has_state = match ctx.db.with(|c| c.query_row("SELECT 1 FROM kv WHERE name=?", [&name], |_| Ok(())).optional()) {
            Ok(r) => r.is_some(),
            Err(e) => return crashed(&e, true),
        };
        let file = widgets.join(format!("{name}.tsx"));
        let src = match std::fs::read(&file) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(_) => return crashed(&format!("ENOENT: no such file or directory, open '{}'", file.display()), true),
        };
        let mut kinds: Vec<&str> = Vec::new();
        if has_state {
            kinds.push("db");
        }
        if src.contains("/data/") {
            kinds.push("api");
        }
        if src.contains("nsh/keymap") {
            kinds.push("keys");
        }
        if kinds.is_empty() {
            kinds.push("mem");
        }
        let title = match titles.get(&name).filter(|t| js::truthy(t)) {
            Some(t) => js::string_of(t),
            None => {
                let raw = w.get("title").filter(|t| js::truthy(t)).map(js::string_of).unwrap_or_else(|| name.clone());
                let raw = raw.replace("**", "");
                let head = raw.split([':', '.', '\n', '—']).next().unwrap_or("");
                js_slice(trim_js(head), 40)
            }
        };
        out.push(json!({
            "name": name,
            "title": if title.is_empty() { name.clone() } else { title },
            "publishedAt": w.get("published_at").cloned().unwrap_or(Value::Null),
            "kinds": kinds,
            "archived": arch.contains(&name),
            "tags": tags.get(&name).cloned().unwrap_or_default(),
        }));
    }
    json_at(200, &json!({"list": out}), true)
}

/// `POST /widget-meta {name, tags?, title?, archived?}`: the Mini Apps tab's
/// own metadata in `meta` (`tags:`, `title:`, `arch:`); always a 200.
async fn widget_meta(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let r = (|| -> Result<Value, String> {
        let j = parse(&req.body).map_err(|e| format!("SyntaxError: {e}"))?;
        let name = get(&j, "name");
        let key = |p: &str| format!("{p}:{}", template(name));
        let set = |k: String, v: &str| ctx.db.with(|c| c.execute("INSERT OR REPLACE INTO meta(k,v) VALUES(?,?)", [k.as_str(), v]).map(|_| ()));
        let del = |k: String| ctx.db.with(|c| c.execute("DELETE FROM meta WHERE k=?", [k]).map(|_| ()));
        // BUG: archived without a name writes the row `arch:undefined` before
        // the name is checked (ART-07); the answer is {ok:false}
        if let Some(Value::Bool(ar)) = get(&j, "archived") {
            if *ar {
                set(key("arch"), "1")?;
            } else {
                del(key("arch"))?;
            }
        }
        let tags = get(&j, "tags");
        if !name.is_some_and(js::truthy) || tags.is_some_and(|t| !t.is_array()) {
            return Ok(json!({"ok": false}));
        }
        if let Some(Value::String(rt)) = get(&j, "title") {
            let clean = js_slice(trim_js(rt), 60);
            if clean.is_empty() {
                del(key("title"))?;
            } else {
                set(key("title"), &clean)?;
            }
        }
        let Some(tags) = tags.and_then(Value::as_array) else { return Ok(json!({"ok": true})) };
        let clean: Vec<String> = tags.iter().map(|x| trim_js(&js::string_of(x)).to_lowercase()).filter(|t| !t.is_empty()).take(8).collect();
        if clean.is_empty() {
            del(key("tags"))?;
        } else {
            set(key("tags"), &clean.join(","))?;
        }
        Ok(json!({"ok": true, "tags": clean}))
    })();
    match r {
        Ok(v) => json_at(200, &v, true),
        Err(e) => json_at(200, &json!({"ok": false, "error": e}), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_locale_compare_orders_as_icu_root() {
        let mut t = vec!["Tally Counter", "report.md", "R4", "R3", "R2"];
        t.sort_by(|a, b| locale_compare(a, b));
        assert_eq!(t, ["R2", "R3", "R4", "report.md", "Tally Counter"]);
        assert_eq!(locale_compare("a", "A"), std::cmp::Ordering::Less);
        assert_eq!(locale_compare("a b", "ab"), std::cmp::Ordering::Less);
    }

    #[test]
    fn apps_bind_refuses_what_node_sqlite_refuses() {
        assert_eq!(bind(None), Err("Provided value cannot be bound to SQLite parameter 1.".to_owned()));
        assert!(bind(Some(&json!({}))).is_err() && bind(Some(&json!(true))).is_err());
        assert_eq!(bind(Some(&json!("R2"))), Ok(SqlValue::Text("R2".into())));
        assert_eq!(bind_number(f64::NAN), SqlValue::Null);
    }
}
