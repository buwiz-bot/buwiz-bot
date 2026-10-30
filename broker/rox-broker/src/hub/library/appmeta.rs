//! A mini app's details (appmeta.mjs): title, file label, description,
//! size and icon in `apps`, the summary `/apps/meta`, `/artifacts` and
//! `/app/info` share, and the icon an image model draws. The routes
//! (`routes/apps.rs`) and the tool loop's app tools (5.4) call these.
//!
//! For the tool loop: `app_info` (app_details_get), `update_app_meta`
//! (app_details_update), `gen_app_icon` (app_icon_generate, and the icon a
//! new app gets), `set_app_cols` and `icon_path`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::types::Value as SqlValue;
use rusqlite::{params_from_iter, OptionalExtension};
use serde_json::{json, Map, Value};

use crate::helper::local::tools;
use crate::hub::ctx::HubCtx;
use crate::hub::library::groups::now36;
use crate::hub::pipeline::versions::{head_of, read_dir_sorted, ver_file, versions_of};
use crate::hub::pipeline::Dirs;
use crate::hub::store::json::{js_number, js_slice, template};
use crate::hub::store::write::to_json;
use crate::hub::store::{now_ms, slug_file, State};
use crate::hub::traits::LocalCommand;
use crate::hub::types::TurnCtx;
use crate::js;

// ————— small JavaScript helpers the library shares —————

/// A JavaScript number as JSON (`NaN` and the infinities as `null`).
pub fn num(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9e15 {
        return json!(f as i64);
    }
    serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null)
}

/// `Math.round(x)`.
pub fn js_round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    (x + 0.5).floor()
}

/// `fs.statSync(p).mtimeMs`.
pub fn mtime_ms(p: &Path) -> Option<f64> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(p).ok()?;
    Some(m.mtime() as f64 * 1000.0 + m.mtime_nsec() as f64 / 1e6)
}

/// `statSync(p).birthtimeMs || statSync(p).mtimeMs`.
pub fn born_ms(p: &Path) -> Option<f64> {
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        let m = std::fs::metadata(p).ok()?;
        let b = m.st_birthtime() as f64 * 1000.0 + m.st_birthtime_nsec() as f64 / 1e6;
        if b != 0.0 {
            return Some(b);
        }
    }
    mtime_ms(p)
}

/// `/^R\d+$/`.
pub fn is_r_name(s: &str) -> bool {
    s.strip_prefix('R').is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

/// `String(s ?? "").trim()` (the JS `trim`).
pub fn trim_js(s: &str) -> &str {
    s.trim_matches(|c: char| js::is_space(c) || matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
}

/// `String(v ?? "")`.
pub fn str_or_empty(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(v) => js::string_of(v),
    }
}

/// A row as node:sqlite's `.get()` hands it to JavaScript, `None` for no row.
pub fn row_of(conn: &rusqlite::Connection, sql: &str, args: &[&dyn rusqlite::ToSql]) -> rusqlite::Result<Option<Map<String, Value>>> {
    let mut st = conn.prepare(sql)?;
    let names: Vec<String> = st.column_names().into_iter().map(str::to_owned).collect();
    st.query_row(args, |r| {
        let mut m = Map::new();
        for (i, n) in names.iter().enumerate() {
            m.insert(n.clone(), to_json(r.get::<_, SqlValue>(i)?));
        }
        Ok(m)
    })
    .optional()
}

/// Every row of `sql` as node:sqlite's `.all()` hands them over.
pub fn rows_of(conn: &rusqlite::Connection, sql: &str, args: &[&dyn rusqlite::ToSql]) -> rusqlite::Result<Vec<Map<String, Value>>> {
    let mut st = conn.prepare(sql)?;
    let names: Vec<String> = st.column_names().into_iter().map(str::to_owned).collect();
    let rows = st.query_map(args, |r| {
        let mut m = Map::new();
        for (i, n) in names.iter().enumerate() {
            m.insert(n.clone(), to_json(r.get::<_, SqlValue>(i)?));
        }
        Ok(m)
    })?;
    rows.collect()
}

fn field<'a>(m: &'a Map<String, Value>, k: &str) -> Option<&'a Value> {
    m.get(k)
}

/// `m[k] || fallback` for a string field.
fn or_str(m: &Map<String, Value>, k: &str, fallback: &str) -> String {
    match m.get(k) {
        Some(v) if js::truthy(v) => js::string_of(v),
        _ => fallback.to_owned(),
    }
}

// ————— paths —————

pub fn gen_dir(ctx: &HubCtx) -> PathBuf {
    ctx.data.join("generated")
}

/// `iconPath(name)`: generated/<name>/icon.png.
pub fn icon_path(ctx: &HubCtx, name: &str) -> PathBuf {
    gen_dir(ctx).join(name).join("icon.png")
}

/// `iconUrl(name, origin)`: the icon's URL with its mtime, "" with none.
pub fn icon_url(ctx: &HubCtx, name: &str, origin: &str) -> String {
    match mtime_ms(&icon_path(ctx, name)) {
        Some(t) => format!("{origin}/app/icon?name={name}&t={}", js::number(js_round(t))),
        None => String::new(),
    }
}

/// `isArtifact(name)`: an R-name with its generated/<name>.tsx.
pub fn is_artifact(ctx: &HubCtx, name: &str) -> bool {
    is_r_name(name) && gen_dir(ctx).join(format!("{name}.tsx")).exists()
}

/// `appRow(name)`: its `apps` row, `{}` with none.
pub fn app_row(ctx: &HubCtx, name: &str) -> Map<String, Value> {
    ctx.db.with(|c| row_of(c, "SELECT * FROM apps WHERE name=?", &[&name])).ok().flatten().unwrap_or_default()
}

/// A JS value as node:sqlite binds it (`undefined` and `null` as NULL).
pub fn sql_of(v: &Value) -> SqlValue {
    match v {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(*b as i64),
        Value::Number(n) => match n.as_i64() {
            Some(i) => SqlValue::Integer(i),
            None => n.as_f64().map(SqlValue::Real).unwrap_or(SqlValue::Null),
        },
        Value::String(s) => SqlValue::Text(s.clone()),
        other => SqlValue::Text(js::string_of(other)),
    }
}

/// `setAppCols(name, cols)`: the upsert that never clobbers a column the
/// caller didn't name.
pub fn set_app_cols(ctx: &HubCtx, name: &str, cols: &[(&str, SqlValue)]) -> Result<(), String> {
    ctx.db.with(|c| {
        c.execute("INSERT INTO apps(name) VALUES(?) ON CONFLICT(name) DO NOTHING", [name])?;
        if !cols.is_empty() {
            let set = cols.iter().map(|(k, _)| format!("{k}=?")).collect::<Vec<_>>().join(",");
            let args: Vec<SqlValue> = cols.iter().map(|(_, v)| v.clone()).chain(std::iter::once(SqlValue::Text(name.to_owned()))).collect();
            c.execute(&format!("UPDATE apps SET {set} WHERE name=?"), params_from_iter(args))?;
        }
        Ok(())
    })
}

// ————— summaries —————

/// A card's declared size and description (`artHints()` entry).
#[derive(Clone, Debug)]
pub struct Hint {
    pub w: Value,
    pub h: Value,
    pub desc: Value,
}

/// `artHints()`: the size hints and description the model declared on each
/// app's cards (turn.art), latest card wins for the size, first for the
/// description.
pub fn art_hints(s: &State) -> HashMap<String, Hint> {
    let mut out: HashMap<String, Hint> = HashMap::new();
    for t in &s.list {
        for u in &t.turns {
            let Some(name) = u.name.as_deref().filter(|n| u.role == "ai" && is_r_name(n)) else { continue };
            let Some(art) = u.art.as_ref().filter(|a| js::truthy(a)) else { continue };
            let cur = out.entry(name.to_owned()).or_insert_with(|| Hint { w: json!(0), h: json!(0), desc: json!("") });
            let (w, h) = (art.get("w").cloned().unwrap_or(Value::Null), art.get("h").cloned().unwrap_or(Value::Null));
            if js::truthy(&w) && js::truthy(&h) {
                cur.w = w;
                cur.h = h;
            }
            if let Some(d) = art.get("desc").filter(|d| js::truthy(d)) {
                if !js::truthy(&cur.desc) {
                    cur.desc = d.clone();
                }
            }
        }
    }
    out
}

/// The hints of the store as it is now.
pub fn hints(ctx: &HubCtx) -> HashMap<String, Hint> {
    ctx.store.read(art_hints)
}

/// The module's own `export const popoutSize = { w, h }`.
pub fn declared_size(ctx: &HubCtx, name: &str) -> Option<(f64, f64)> {
    static RE: OnceLock<regex_lite::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex_lite::Regex::new(r"export\s+const\s+popoutSize\s*=\s*\{[^}]*?w\s*:\s*(\d+(?:\.\d+)?)[^}]*?h\s*:\s*(\d+(?:\.\d+)?)").expect("the size pattern"));
    let src = std::fs::read_to_string(gen_dir(ctx).join(format!("{name}.tsx"))).ok()?;
    let m = re.captures(&src)?;
    Some((m[1].parse().ok()?, m[2].parse().ok()?))
}

/// `a || b || …`: the first truthy value, else the last.
fn first_truthy(vals: Vec<Value>) -> Value {
    let last = vals.last().cloned().unwrap_or(Value::Null);
    vals.into_iter().find(js::truthy).unwrap_or(last)
}

/// `appSummary(name, origin, row, hints)`: what /apps/meta, /artifacts and
/// /app/info share.
pub fn app_summary(ctx: &HubCtx, name: &str, origin: &str, row: &Map<String, Value>, hints: &HashMap<String, Hint>) -> Map<String, Value> {
    let hint = hints.get(name);
    let truthy = |k: &str| row.get(k).is_some_and(js::truthy);
    let decl = if truthy("w") && truthy("h") { None } else { declared_size(ctx, name) };
    let vers: Value = ctx.db.with(|c| c.query_row("SELECT MAX(created_at) AS m FROM versions WHERE name=?", [name], |r| r.get::<_, SqlValue>(0)).map(to_json)).unwrap_or(Value::Null);
    let description = match row.get("description") {
        Some(v) if !v.is_null() => v.clone(),
        _ => hint.map(|h| h.desc.clone()).filter(|d| !d.is_null()).unwrap_or(json!("")),
    };
    let size = |k: &str, d: Option<f64>| {
        let mut vals = vec![num(js_number(row.get(k).or(Some(&Value::Null))))];
        vals[0] = if vals[0].is_null() { json!(0) } else { vals[0].clone() };
        vals.push(hint.map(|h| if k == "w" { h.w.clone() } else { h.h.clone() }).unwrap_or(Value::Null));
        vals.push(d.map(num).unwrap_or(Value::Null));
        vals.push(json!(0));
        first_truthy(vals)
    };
    let nz = |v: Option<&Value>| {
        let n = js_number(v.or(Some(&Value::Null)));
        if n.is_nan() {
            0.0
        } else {
            n
        }
    };
    let updated = js_round(nz(row.get("updated_at")).max(nz(Some(&vers))));
    let mut m = Map::new();
    m.insert("name".into(), json!(name));
    m.insert("title".into(), json!(or_str(row, "title", name)));
    m.insert("file".into(), json!(or_str(row, "file", &format!("{name}.tsx"))));
    m.insert("description".into(), description);
    m.insert("icon".into(), json!(icon_url(ctx, name, origin)));
    m.insert("w".into(), size("w", decl.map(|d| d.0)));
    m.insert("h".into(), size("h", decl.map(|d| d.1)));
    m.insert("v".into(), head_of(ctx, name).ok().flatten().map_or(Value::Null, |v| json!(v)));
    m.insert("updated_at".into(), if updated != 0.0 && !updated.is_nan() { num(updated) } else { Value::Null });
    m
}

/// `appsMeta(origin)`: every live app in generated/, numerically.
pub fn apps_meta(ctx: &HubCtx, origin: &str) -> Result<Vec<Value>, String> {
    let (tomb, rows) = ctx.db.with(|c| {
        let tomb: std::collections::HashSet<String> = rows_of(c, "SELECT name FROM tombstones", &[])?.iter().map(|r| r.get("name").map(js::string_of).unwrap_or_default()).collect();
        let rows: HashMap<String, Map<String, Value>> = rows_of(c, "SELECT * FROM apps", &[])?.into_iter().map(|r| (r.get("name").map(js::string_of).unwrap_or_default(), r)).collect();
        Ok((tomb, rows))
    })?;
    let hints = hints(ctx);
    let list = read_dir_sorted(&gen_dir(ctx)).map_err(|e| e.to_string())?;
    let mut names: Vec<String> =
        list.iter().filter_map(|e| e.file_name().to_str().and_then(|f| f.strip_suffix(".tsx")).filter(|n| is_r_name(n)).map(str::to_owned)).filter(|n| !tomb.contains(n)).collect();
    names.sort_by(|a, b| a[1..].parse::<f64>().unwrap_or(0.0).total_cmp(&b[1..].parse::<f64>().unwrap_or(0.0)));
    let empty = Map::new();
    Ok(names.iter().map(|n| Value::Object(app_summary(ctx, n, origin, rows.get(n).unwrap_or(&empty), &hints))).collect())
}

/// `appInfo(name, origin)`: everything the details modal shows.
pub fn app_info(ctx: &HubCtx, name: &str, origin: &str) -> Result<Value, String> {
    if !is_artifact(ctx, name) {
        return Err(format!("no such app {name}"));
    }
    let row = app_row(ctx, name);
    let base = app_summary(ctx, name, origin, &row, &hints(ctx));
    type Stats = (Value, Value, Option<String>, Option<String>, Option<String>, Option<String>);
    let (n, c, first, thread_id, thread_title, bot): Stats = ctx.db.with(|db| {
        let (n, c) = db.query_row("SELECT COUNT(*) AS n, MIN(created_at) AS c FROM versions WHERE name=?", [name], |r| Ok((to_json(r.get(0)?), to_json(r.get(1)?))))?;
        let first = db.query_row("SELECT prompt FROM versions WHERE name=? ORDER BY v LIMIT 1", [name], |r| r.get::<_, Option<String>>(0)).optional()?.flatten();
        let tid = db
            .query_row("SELECT thread_id FROM turns WHERE name=? AND role='ai' ORDER BY uid, seq LIMIT 1", [name], |r| r.get::<_, SqlValue>(0))
            .optional()?
            .map(to_json)
            .filter(js::truthy)
            .map(|v| js::string_of(&v));
        let (title, bot) = match &tid {
            Some(t) => (
                db.query_row("SELECT title FROM threads WHERE id=?", [t], |r| r.get::<_, SqlValue>(0)).optional()?.map(to_json).filter(js::truthy).map(|v| js::string_of(&v)),
                db.query_row("SELECT name FROM bots WHERE thread_id=?", [t], |r| r.get::<_, SqlValue>(0)).optional()?.map(to_json).filter(js::truthy).map(|v| js::string_of(&v)),
            ),
            None => (None, None),
        };
        Ok((n, c, first, tid, title, bot))
    })?;
    let dirs = Dirs::of(&ctx.data);
    let v = base.get("v").and_then(Value::as_i64).filter(|v| *v != 0);
    let path = v.and_then(|v| ver_file(&dirs, name, &v.to_string())).unwrap_or_else(|| dirs.gen.join(format!("{name}.tsx")));
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let created = js_round(js_number(Some(&c)));
    let published = ctx.db.with(|db| db.query_row("SELECT 1 FROM widgets WHERE name=?", [name], |_| Ok(())).optional().map(|r| r.is_some()))?;
    let mut m = Map::new();
    m.insert("ok".into(), json!(true));
    m.insert("generating".into(), json!(icon_generating(ctx, name)));
    m.insert("icon_error".into(), json!(icon_error(ctx, name)));
    m.extend(base);
    m.insert("icon_prompt".into(), json!(or_str(&row, "icon_prompt", "")));
    m.insert("versions".into(), num(js_number(Some(&n))));
    m.insert("history".into(), Value::Array(versions_of(ctx, name)?));
    m.insert("created_at".into(), if created != 0.0 && !created.is_nan() { num(created) } else { Value::Null });
    m.insert("prompt".into(), json!(first.filter(|p| !p.is_empty()).unwrap_or_default()));
    m.insert("path".into(), json!(path.display().to_string()));
    m.insert("bytes".into(), json!(bytes));
    m.insert("published".into(), json!(published));
    m.insert("thread_id".into(), json!(thread_id.unwrap_or_default()));
    m.insert("thread_title".into(), json!(thread_title.unwrap_or_default()));
    m.insert("bot_name".into(), json!(bot.unwrap_or_default()));
    Ok(Value::Object(m))
}

// ————— edits —————

/// `fromUri(src)`: a file:// URI as its path.
fn from_uri(src: &str) -> Result<String, String> {
    let p = trim_js(src);
    match p.strip_prefix("file://") {
        Some(rest) => js::decode_uri_component(rest).map_err(|_| "URI malformed".to_owned()),
        None => Ok(p.to_owned()),
    }
}

/// `adoptIcon(name, src, out)`: any image sips reads becomes the square
/// 512×512 png at `out`. → `out`.
pub async fn adopt_icon(ctx: &HubCtx, src: &str, out: &Path) -> Result<String, String> {
    let p = from_uri(src)?;
    if !p.starts_with('/') || !Path::new(&p).exists() {
        return Err(format!("no such image {src}"));
    }
    let out_s = out.display().to_string();
    let tmp = format!("{out_s}.{}.tmp.png", now36());
    let cmd = LocalCommand { program: "sips".into(), args: ["-s", "format", "png", "-z", "512", "512", &p, "--out", &tmp].map(str::to_owned).to_vec(), ..Default::default() };
    let ran = tools(ctx).run(cmd).await;
    let ok = matches!(ran, Ok(ref o) if o.code == Some(0)) && std::fs::rename(&tmp, out).is_ok();
    if !ok {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("could not convert {p} to a png"));
    }
    Ok(out_s)
}

/// `clampSize(n)`: null for none or 0, else 120…2400.
fn clamp_size(n: &Value) -> Result<SqlValue, String> {
    if n.is_null() || n.as_str() == Some("") || js_number(Some(n)) == 0.0 {
        return Ok(SqlValue::Null);
    }
    let v = js_number(Some(n));
    if !v.is_finite() {
        return Err(format!("bad size {}", js::string_of(n)));
    }
    Ok(SqlValue::Real(js_round(v).clamp(120.0, 2400.0)))
}

fn strip_tsx(s: &str) -> &str {
    match s.len().checked_sub(4).and_then(|k| s.get(k..).map(|t| (k, t))) {
        Some((k, t)) if t.eq_ignore_ascii_case(".tsx") => &s[..k],
        _ => s,
    }
}

/// `slugFile(x)` of a JS value (`String(s || "")`).
fn slug_of(v: Option<&Value>) -> String {
    match v {
        Some(v) if js::truthy(v) => slug_file(&js::string_of(v)),
        _ => String::new(),
    }
}

/// `updateAppMeta(name, {title, file, description, w, h, icon})`: every
/// field optional (absent keys keep their values). A new file label or
/// title renames every version file to the new slug.
pub async fn update_app_meta(ctx: &HubCtx, name: &str, body: &Value) -> Result<(), String> {
    if !is_artifact(ctx, name) {
        return Err(format!("no such module {name}"));
    }
    let get = |k: &str| body.as_object().and_then(|o| o.get(k));
    let cur = app_row(ctx, name);
    let mut cols: Vec<(&str, SqlValue)> = vec![("updated_at", SqlValue::Integer(now_ms() as i64))];
    let t: Option<String> = match get("title") {
        None => field(&cur, "title").filter(|v| !v.is_null()).map(js::string_of),
        Some(v) => Some(js_slice(trim_js(&str_or_empty(Some(v))), 60)).filter(|s| !s.is_empty()),
    };
    let f: Option<String> = match get("file") {
        None => field(&cur, "file").filter(|v| !v.is_null()).map(js::string_of),
        Some(v) => Some(slug_file(&str_or_empty(Some(v)))).filter(|s| !s.is_empty()),
    };
    cols.push(("title", t.clone().map_or(SqlValue::Null, SqlValue::Text)));
    cols.push(("file", f.clone().map_or(SqlValue::Null, SqlValue::Text)));
    if let Some(d) = get("description") {
        cols.push(("description", SqlValue::Text(js_slice(trim_js(&str_or_empty(Some(d))), 280))));
    }
    if let Some(w) = get("w") {
        cols.push(("w", clamp_size(w)?));
    }
    if let Some(h) = get("h") {
        cols.push(("h", clamp_size(h)?));
    }
    if let Some(icon) = get("icon") {
        if trim_js(&str_or_empty(Some(icon))).is_empty() {
            let _ = std::fs::remove_file(icon_path(ctx, name));
            cols.push(("icon", SqlValue::Null));
            cols.push(("icon_prompt", SqlValue::Null));
        } else {
            let out = adopt_icon(ctx, &js::string_of(icon), &icon_path(ctx, name)).await?;
            cols.push(("icon", SqlValue::Text(out)));
            cols.push(("icon_prompt", SqlValue::Null));
        }
    }
    set_app_cols(ctx, name, &cols)?;
    let pick = |a: Option<String>, b: String| a.filter(|s| !s.is_empty()).or_else(|| Some(b).filter(|s| !s.is_empty())).unwrap_or_else(|| format!("{name}.tsx"));
    let old = pick(field(&cur, "file").filter(|v| js::truthy(v)).map(js::string_of), slug_of(field(&cur, "title")));
    let new = pick(f, t.as_deref().map(slug_file).unwrap_or_default());
    let (old, new) = (strip_tsx(&old), strip_tsx(&new));
    if new != old {
        let dir = gen_dir(ctx).join(name);
        if let Ok(list) = read_dir_sorted(&dir) {
            for e in list {
                let x = e.file_name().to_string_lossy().into_owned();
                if let Some(v) = version_of_file(&x) {
                    if std::fs::rename(dir.join(&x), dir.join(format!("{new}.v{v}.tsx"))).is_err() {
                        break;
                    }
                }
            }
        }
        ctx.store.write_index();
    }
    Ok(())
}

/// `x.match(/\.v(\d+)\.tsx$/)?.[1]`.
pub fn version_of_file(x: &str) -> Option<&str> {
    let stem = x.strip_suffix(".tsx")?;
    let at = stem.rfind(".v")?;
    let d = &stem[at + 2..];
    (!d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())).then_some(d)
}

// ————— generated icons —————

/// `iconSubject(name, prompt)`: what the icon depicts.
fn icon_subject(ctx: &HubCtx, name: &str, prompt: &str) -> String {
    let p = trim_js(prompt);
    if !p.is_empty() {
        return js_slice(p, 600);
    }
    let row = app_row(ctx, name);
    let desc = match row.get("description") {
        Some(v) if !v.is_null() => js::string_of(v),
        _ => hints(ctx).get(name).map(|h| str_or_empty(Some(&h.desc))).unwrap_or_default(),
    };
    let first =
        ctx.db.with(|c| c.query_row("SELECT prompt FROM versions WHERE name=? ORDER BY v LIMIT 1", [name], |r| r.get::<_, Option<String>>(0)).optional()).ok().flatten().flatten().unwrap_or_default();
    let parts = [
        or_str(&row, "title", name),
        if desc.is_empty() { String::new() } else { format!("— {desc}") },
        if first.is_empty() { String::new() } else { format!("(an app built for: {})", js_slice(&first, 200)) },
    ];
    parts.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")
}

const MAGICK: [&str; 2] = ["/opt/homebrew/bin/magick", "/usr/local/bin/magick"];

/// `normalizeIcon(file)`: a shape on a white or empty canvas gets
/// transparent corners and is trimmed to fill the square; a no-op without
/// ImageMagick.
pub async fn normalize_icon(ctx: &HubCtx, file: &Path) {
    let Some(magick) = MAGICK.iter().find(|p| Path::new(p).exists()) else { return };
    let f = file.display().to_string();
    let run = |args: Vec<String>| LocalCommand { program: (*magick).to_owned(), args, ..Default::default() };
    let corners = run(["-alpha", "set", "-format", "%[pixel:p{1,1}] %[pixel:p{510,1}] %[pixel:p{1,510}] %[pixel:p{510,510}]", "info:"].iter().fold(vec![f.clone()], |mut a, s| {
        a.push((*s).to_owned());
        a
    }));
    let out = match tools(ctx).run(corners).await {
        Ok(o) if o.code == Some(0) => String::from_utf8_lossy(&o.stdout).into_owned(),
        Ok(o) => return eprintln!("  icon normalize failed: Command failed: {magick}\n{}", String::from_utf8_lossy(&o.stderr)),
        Err(e) => return eprintln!("  icon normalize failed: spawn {magick} {e}"),
    };
    let blank = |c: &str| {
        let n: Vec<f64> = c.split(|ch: char| !(ch.is_ascii_digit() || ch == '.')).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect();
        let (r, g, b, a) = (n.first().copied().unwrap_or(f64::NAN), n.get(1).copied().unwrap_or(f64::NAN), n.get(2).copied().unwrap_or(f64::NAN), n.get(3).copied().unwrap_or(1.0));
        a < 0.1 || (r > 235.0 && g > 235.0 && b > 235.0)
    };
    if !trim_js(&out).split(' ').any(blank) {
        return;
    }
    let mut args = vec![f.clone(), "-alpha".into(), "set".into(), "-fuzz".into(), "8%".into(), "-fill".into(), "none".into()];
    for (x, y) in [(0, 0), (511, 0), (0, 511), (511, 511)] {
        args.push("-draw".into());
        args.push(format!("alpha {x},{y} floodfill"));
    }
    args.extend(["-trim", "+repage", "-background", "none", "-gravity", "center", "-resize", "512x512^", "-extent", "512x512"].map(str::to_owned));
    args.push(f);
    if let Ok(o) = tools(ctx).run(run(args)).await {
        if o.code != Some(0) {
            eprintln!("  icon normalize failed: Command failed: {magick}");
        }
    }
}

type IconRun = tokio::sync::watch::Receiver<Option<Result<String, String>>>;

/// `inflight` and `iconErrors`, by data dir and name.
#[derive(Default)]
struct IconJobs {
    inflight: HashMap<(PathBuf, String), IconRun>,
    errors: HashMap<(PathBuf, String), String>,
}

fn icon_jobs() -> &'static Mutex<IconJobs> {
    static J: OnceLock<Mutex<IconJobs>> = OnceLock::new();
    J.get_or_init(Default::default)
}

/// `iconGenerating(name)`.
pub fn icon_generating(ctx: &HubCtx, name: &str) -> bool {
    icon_jobs().lock().unwrap_or_else(|e| e.into_inner()).inflight.contains_key(&(ctx.data.clone(), name.to_owned()))
}

/// `iconError(name)`: the last generation's failure, "" with none.
pub fn icon_error(ctx: &HubCtx, name: &str) -> String {
    icon_jobs().lock().unwrap_or_else(|e| e.into_inner()).errors.get(&(ctx.data.clone(), name.to_owned())).cloned().unwrap_or_default()
}

/// `imageCapable(model)` against the configured opencodex.
pub async fn image_capable(model: &str) -> bool {
    crate::hub::model::images::image_capable(&crate::hub::model::Settings::global().opencodex, model).await
}

/// `genAppIcon(name, prompt, model)`: draws the app's icon with an image
/// model `model` reaches. Started at once (the run is the app's until it
/// ends; a second call joins it); the future answers the backend that drew
/// it. `/app/info` shows `generating` and `icon_error` meanwhile.
pub fn gen_app_icon(ctx: Arc<HubCtx>, name: &str, prompt: &str, model: &str) -> impl std::future::Future<Output = Result<String, String>> + Send + 'static {
    let key = (ctx.data.clone(), name.to_owned());
    let rx: Result<IconRun, String> = if !is_artifact(&ctx, name) {
        Err(format!("no such app {name}"))
    } else {
        let mut jobs = icon_jobs().lock().unwrap_or_else(|e| e.into_inner());
        match jobs.inflight.get(&key) {
            Some(rx) => Ok(rx.clone()),
            None => {
                jobs.errors.remove(&key);
                let (tx, rx) = tokio::sync::watch::channel(None);
                jobs.inflight.insert(key.clone(), rx.clone());
                let (name, prompt, model) = (name.to_owned(), prompt.to_owned(), model.to_owned());
                tokio::spawn(async move {
                    let r = draw_icon(&ctx, &name, &prompt, &model).await;
                    let mut jobs = icon_jobs().lock().unwrap_or_else(|e| e.into_inner());
                    if let Err(e) = &r {
                        jobs.errors.insert(key.clone(), js_slice(e, 200));
                    }
                    jobs.inflight.remove(&key);
                    drop(jobs);
                    let _ = tx.send(Some(r));
                });
                Ok(rx)
            }
        }
    };
    async move {
        let mut rx = rx?;
        loop {
            if let Some(r) = rx.borrow().clone() {
                return r;
            }
            if rx.changed().await.is_err() {
                return rx.borrow().clone().unwrap_or_else(|| Err("icon generation ended".to_owned()));
            }
        }
    }
}

async fn draw_icon(ctx: &Arc<HubCtx>, name: &str, prompt: &str, model: &str) -> Result<String, String> {
    if !image_capable(model).await {
        return Err(format!("{} can't generate images", if model.is_empty() { "this model" } else { model }));
    }
    let subject = icon_subject(ctx, name, prompt);
    let out = icon_path(ctx, name);
    let out_s = out.display().to_string();
    let tmp = PathBuf::from(format!("{out_s}.{}.gen.png", now36()));
    let r = async {
        let api = ctx.model.clone().ok_or_else(|| "no model client".to_owned())?;
        let opts = crate::hub::model::images::ImageOpts {
            background: Some("opaque".into()),
            svg_hint: Some(format!("Draw app icon artwork as a single SVG with viewBox=\"0 0 512 512\". The first element must be <rect width=\"512\" height=\"512\"/> filled with a vivid gradient so the background covers every pixel, corners included (no rx/ry, no rounded-square or squircle shape, no frame, no margin; the app applies its own corner mask). On it, a single bold centered symbol with soft depth and lighting, kept inside the central 360×360, representing: {subject}. No text, no letters, no border, no drop shadow.")),
            ..Default::default()
        };
        let backend = crate::hub::model::images::generate_image(
            crate::hub::model::Settings::global(),
            api.as_ref(),
            &TurnCtx::default(),
            tools(ctx).as_ref(),
            &format!("App icon artwork, a perfect square filled edge to edge: a vivid gradient background that reaches all four edges and corners, with a single bold centered symbol representing: {subject}. The symbol stays inside the central 70% of the square. Do not draw a rounded-square, squircle, tile, frame or device; no white or empty margins, no text, no letters, no border, no drop shadow."),
            &tmp,
            &opts,
        )
        .await?;
        let tmp_s = tmp.display().to_string();
        let cmd = LocalCommand { program: "sips".into(), args: ["-s", "format", "png", "-z", "512", "512", &tmp_s, "--out", &out_s].map(str::to_owned).to_vec(), ..Default::default() };
        match tools(ctx).run(cmd).await {
            Ok(o) if o.code == Some(0) => {}
            Ok(o) => return Err(format!("Command failed: sips -s format png -z 512 512 {tmp_s} --out {out_s}\n{}", String::from_utf8_lossy(&o.stderr))),
            Err(e) => return Err(format!("spawn sips {e}")),
        }
        normalize_icon(ctx, &out).await;
        set_app_cols(ctx, name, &[("icon", SqlValue::Text(out_s.clone())), ("icon_prompt", SqlValue::Text(subject.clone())), ("updated_at", SqlValue::Integer(now_ms() as i64))])?;
        eprintln!("  icon {name} via {backend}");
        Ok(backend)
    }
    .await;
    let _ = std::fs::remove_file(&tmp);
    r
}

/// The tool loop's side of the Library (5.4's [`AppMetaSeam`]): `app_icon`,
/// `app_details_update` and a new card's icon.
///
/// [`AppMetaSeam`]: crate::hub::tools::artifact::AppMetaSeam
pub struct ToolSide;

impl crate::hub::tools::artifact::AppMeta for ToolSide {
    fn gen_icon(&self, ctx: Arc<HubCtx>, name: String, prompt: String, model: String) -> crate::hub::types::BoxFuture<'static, Result<String, String>> {
        Box::pin(gen_app_icon(ctx, &name, &prompt, &model))
    }

    /// tools.mjs: `updateAppMeta(String(name || ""), meta)` then
    /// `appInfo(String(name))` (no origin: the answer reads no URL).
    fn update(&self, ctx: Arc<HubCtx>, name: String, meta: Value) -> crate::hub::types::BoxFuture<'static, Result<Value, String>> {
        Box::pin(async move {
            update_app_meta(&ctx, &name, &meta).await?;
            app_info(&ctx, &name, "undefined")
        })
    }
}

/// `${v}` of a body field, for messages.
pub fn shown(v: Option<&Value>) -> String {
    template(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appmeta_helpers_read_as_javascript() {
        assert!(is_r_name("R12") && !is_r_name("R") && !is_r_name("r1") && !is_r_name("R1a"));
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(version_of_file("tally.v12.tsx"), Some("12"));
        assert_eq!(version_of_file("v1.tsx"), None);
        assert_eq!(version_of_file("a.v.tsx"), None);
        assert_eq!(clamp_size(&json!(50)).unwrap(), SqlValue::Real(120.0));
        assert_eq!(clamp_size(&json!(5000)).unwrap(), SqlValue::Real(2400.0));
        assert_eq!(clamp_size(&json!(0)).unwrap(), SqlValue::Null);
        assert_eq!(clamp_size(&json!("")).unwrap(), SqlValue::Null);
        assert_eq!(clamp_size(&json!("wide")), Err("bad size wide".to_owned()));
        assert_eq!(js::stringify(&num(400.0)), "400");
        assert_eq!(js::stringify(&num(1.5)), "1.5");
    }
}
