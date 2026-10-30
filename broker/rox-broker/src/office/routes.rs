//! The office routes (routes/files.mjs, files.mjs' `renderFile` for office
//! kinds, office.mjs's caps and vendor roots), answered here with the hub
//! on:
//!
//! - `GET /office/`, `/office/?…`: the viewer page (viewer.html);
//!   `GET /office/editor/…`: the editor page, its bundle URL stamped with
//!   the bundle's mtime;
//! - `GET /office/dist/<file>`: the editor bundle (`office/editor/dist`,
//!   built once when packaging);
//! - `GET /office/vendor/<pkg>/<file>`: the installed `@betteroffice/*`
//!   packages' files the pages import;
//! - `GET /office/caps`: which kinds have their package;
//! - `GET /file/render?path=<a .docx, .xlsx or .pptx>`: the viewer's payload.
//!
//! The pages and bundles are static browser files; the server dir is
//! Node's (`--node-script`'s), where they are packaged today.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{HeaderValue, CACHE_CONTROL, CONTENT_TYPE};
use hyper::{Response, StatusCode};
use serde_json::{json, Map, Value};

use crate::hub::ctx::HubCtx;
use crate::hub::types::{json as json_response, HubRequest, HubResponse, Pattern, Verb};
use crate::hub::Registry;

/// Node's `src` dir: what `serverFile()` resolves against.
fn src_dir() -> PathBuf {
    crate::hub::market::node_paths().0
}

fn respond(status: StatusCode, body: Vec<u8>, content_type: Option<&str>, no_cache: bool) -> HubResponse {
    let mut res = Response::new(Full::new(Bytes::from(body)).map_err(|never| match never {}).boxed());
    *res.status_mut() = status;
    if let Some(t) = content_type {
        res.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_str(t).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    }
    if no_cache {
        res.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }
    res
}

fn not_found() -> HubResponse {
    respond(StatusCode::NOT_FOUND, Vec::new(), None, false)
}

/// office.mjs's `VENDOR_PKGS`.
fn vendor_package(key: &str) -> Option<&'static str> {
    Some(match key {
        "docx" => "@betteroffice/docx",
        "xlsx" => "@betteroffice/xlsx",
        "pptx" => "@betteroffice/pptx",
        "fonts" => "@betteroffice/fonts",
        "docx-react" => "@betteroffice/docx-react",
        "xlsx-react" => "@betteroffice/xlsx-react",
        "pptx-react" => "@betteroffice/pptx-react",
        _ => return None,
    })
}

/// `pkgRootDir(name)`: the installed package's dir, if it is installed.
fn package_root(name: &str) -> Option<PathBuf> {
    let dir = src_dir().join("..").join("node_modules").join(name);
    let dir = std::fs::canonicalize(dir).ok()?;
    dir.join("package.json").is_file().then_some(dir)
}

/// `officeCaps()`: a kind is viewable when its engine package is installed.
pub fn caps() -> Value {
    json!({ "docx": package_root("@betteroffice/docx").is_some(), "xlsx": package_root("@betteroffice/xlsx").is_some(), "pptx": package_root("@betteroffice/pptx").is_some() })
}

/// A file under `root` by its request path (`..` folded as `path.join`
/// folds it), when it stays inside and is a file.
fn file_under(root: &Path, rest: &str) -> Option<PathBuf> {
    let p = super::workspace::resolve(root, rest);
    (p.starts_with(root) && p != root && p.is_file()).then_some(p)
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn editor_bundle() -> PathBuf {
    src_dir().join("office").join("editor").join("dist").join("editor.js")
}

fn mtime_ms(p: &Path) -> Option<u64> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(m.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64)
}

fn rest_of(url: &str, prefix: &str) -> String {
    let path = url.split('?').next().unwrap_or("");
    let raw = &path[prefix.len().min(path.len())..];
    crate::js::decode_uri_component(raw).unwrap_or_else(|_| raw.to_owned())
}

async fn office(req: HubRequest) -> HubResponse {
    let url = req.url.as_str();
    let path = req.path();
    if path == "/office/" || path.starts_with("/office/editor/") {
        let editing = path.starts_with("/office/editor/");
        let page = src_dir().join("office").join(if editing { "editor/editor.html" } else { "viewer.html" });
        let Ok(mut html) = std::fs::read_to_string(&page) else {
            return json_response(StatusCode::INTERNAL_SERVER_ERROR, &json!({ "error": format!("ENOENT: no such file or directory, open '{}'", page.display()) }));
        };
        if editing {
            let v = mtime_ms(&editor_bundle()).unwrap_or(0);
            html = html.replace("/office/dist/editor.js", &format!("/office/dist/editor.js?v={v}"));
        }
        return respond(StatusCode::OK, html.into_bytes(), Some("text/html; charset=utf-8"), true);
    }
    if url.starts_with("/office/dist/") {
        let dir = src_dir().join("office").join("editor").join("dist");
        let Some(p) = file_under(&dir, &rest_of(url, "/office/dist/")) else { return not_found() };
        let ext = ext_of(&p);
        let t = match ext.as_str() {
            "js" | "mjs" => "text/javascript",
            "wasm" => "application/wasm",
            "css" => "text/css",
            "ttf" => "font/ttf",
            "otf" => "font/otf",
            "map" => "application/json",
            _ => "application/octet-stream",
        };
        return match std::fs::read(&p) {
            Ok(b) => respond(StatusCode::OK, b, Some(t), true),
            Err(_) => not_found(),
        };
    }
    if url == "/office/caps" {
        return json_response(StatusCode::OK, &caps());
    }
    if url.starts_with("/office/vendor/") {
        let rest = rest_of(url, "/office/vendor/");
        let (key, segs) = rest.split_once('/').unwrap_or((rest.as_str(), ""));
        let Some(root) = vendor_package(key).and_then(package_root) else { return not_found() };
        let Some(p) = file_under(&root, segs) else { return not_found() };
        let t = match ext_of(&p).as_str() {
            "js" | "mjs" => "text/javascript",
            "css" => "text/css",
            "wasm" => "application/wasm",
            "ttf" => "font/ttf",
            "otf" => "font/otf",
            "woff" => "font/woff",
            "woff2" => "font/woff2",
            "json" | "map" => "application/json",
            _ => "application/octet-stream",
        };
        return match std::fs::read(&p) {
            Ok(b) => respond(StatusCode::OK, b, Some(t), true),
            Err(_) => not_found(),
        };
    }
    // no office route: Node's catch-all
    respond(StatusCode::NOT_FOUND, b"not found".to_vec(), None, false)
}

/// The kinds `/file/render` answers here: the ones Node reads with its
/// office libraries (mammoth, ExcelJS) or hands to the office viewer.
pub fn renders(url: &str) -> bool {
    let rel = crate::js::search_param(url, "path").unwrap_or_default();
    let ext = Path::new(&rel).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    !rel.starts_with("app:") && ["docx", "xlsx", "pptx"].contains(&ext.as_str())
}

/// files.mjs's `cardVariant`/`viewerKind` for the office kinds.
fn kind_variant(ext: &str) -> (&'static str, &'static str) {
    match ext {
        "docx" => ("docx", "docx"),
        "xlsx" => ("table", "spreadsheet"),
        _ => ("pptx", "ppt"),
    }
}

/// files.mjs's `officeUrl(path, rel, ext)`.
fn office_url(rel: &str, ext: &str) -> Option<String> {
    let kind = ext;
    if rel.is_empty() || caps()[kind] != json!(true) {
        return None;
    }
    let bundle = editor_bundle();
    let editing = bundle.exists();
    let page = if editing { "/office/editor/" } else { "/office/" };
    let v = if editing { format!("&v={}", mtime_ms(&bundle).unwrap_or(0)) } else { String::new() };
    let path: Vec<String> = rel.split('/').map(|s| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>().replace('+', "%20")).map(|s| encode_component(&s)).collect();
    Some(format!("{page}?path={}&kind={kind}{v}", path.join("/")))
}

/// `encodeURIComponent` over `byte_serialize`'s output: it leaves the
/// characters JavaScript leaves (`A-Z a-z 0-9 - _ . ! ~ * ' ( )`).
fn encode_component(s: &str) -> String {
    s.replace("%21", "!").replace("%7E", "~").replace("%27", "'").replace("%28", "(").replace("%29", ")").replace("%2A", "*")
}

pub(crate) async fn render(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let rel = req.query("path").unwrap_or_default();
    let Some(p) = crate::hub::tools::office::out_file(&ctx, &rel) else {
        return json_response(StatusCode::OK, &json!({ "error": "bad path" }));
    };
    let base = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // fileMeta(path, name without an upload prefix)
    let name = regex_lite::Regex::new(r"^[a-z0-9]+-[a-z0-9]{4}-").map(|re| re.replace(&base, "").into_owned()).unwrap_or(base);
    let ext = Path::new(&name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let (kind, variant) = kind_variant(&ext);
    let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    let mut out = Map::new();
    out.insert("name".into(), json!(name));
    out.insert("ext".into(), json!(ext));
    out.insert("size".into(), json!(size));
    out.insert("kind".into(), json!(kind));
    out.insert("variant".into(), json!(variant));
    let url = office_url(&rel, &ext);
    if !p.exists() {
        out.insert("error".into(), json!("missing"));
        return json_response(StatusCode::OK, &Value::Object(out));
    }
    let content = tokio::task::spawn_blocking(move || match ext.as_str() {
        "docx" => markdown::docx(&p).map(|t| ("text", json!(t))),
        "xlsx" => markdown::sheets(&p).map(|s| ("sheets", s)),
        _ => Ok(("", Value::Null)),
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    match content {
        Ok((key, value)) => {
            if !key.is_empty() {
                out.insert(key.into(), value);
            }
            if let Some(u) = url {
                out.insert("office".into(), json!(u));
            }
            json_response(StatusCode::OK, &Value::Object(out))
        }
        Err(e) => json_response(StatusCode::OK, &json!({ "error": e })),
    }
}

pub fn install(routes: &mut Registry) {
    routes.route(Verb::Get, Pattern::Prefix("/office/"), |_, req| office(req));
    routes.route(Verb::Get, Pattern::Exact("/file/render"), render);
}

/// The viewer's text for a docx and rows for a workbook, as files.mjs reads
/// them with mammoth (through `htmlToMd`) and ExcelJS.
pub mod markdown {
    use std::path::Path;

    use serde_json::{json, Value};

    fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
        v.get(key).filter(|x| !x.is_null())
    }

    struct Ctx {
        /// style id → name
        styles: Vec<(String, String)>,
        /// (numId, ilvl) → ordered
        numbering: Value,
    }

    impl Ctx {
        fn heading(&self, style: &str) -> Option<usize> {
            let name = self.styles.iter().find(|(id, _)| id == style).map(|(_, n)| n.to_lowercase())?;
            let n = name.strip_prefix("heading ")?.parse::<usize>().ok()?;
            (1..=6).contains(&n).then_some(n)
        }

        fn ordered(&self, num_id: &Value, level: u64) -> bool {
            let num_id = crate::js::string_of(num_id);
            let nums = self.numbering.get("nums").and_then(Value::as_array).cloned().unwrap_or_default();
            let Some(num) = nums.iter().find(|n| n.get("numId").map(crate::js::string_of).as_deref() == Some(num_id.as_str())) else { return false };
            let abs = num.get("abstractNumId").map(crate::js::string_of).unwrap_or_default();
            let abstracts = self.numbering.get("abstractNums").and_then(Value::as_array).cloned().unwrap_or_default();
            let Some(a) = abstracts.iter().find(|x| x.get("abstractNumId").map(crate::js::string_of).as_deref() == Some(abs.as_str())) else { return false };
            let levels = a.get("levels").and_then(Value::as_array).cloned().unwrap_or_default();
            let lvl = levels.iter().find(|l| l.get("ilvl").and_then(Value::as_f64) == Some(level as f64)).or(levels.first());
            lvl.and_then(|l| l.get("numFmt")).and_then(Value::as_str).is_some_and(|f| f != "bullet")
        }
    }

    /// One inline run of text with mammoth's marks, as `inline()` turns them.
    fn inline(content: &Value, out: &mut String) {
        for node in content.as_array().into_iter().flatten() {
            match node.get("type").and_then(Value::as_str).unwrap_or("") {
                "run" => {
                    let f = node.get("formatting").cloned().unwrap_or(Value::Null);
                    let bold = f.get("bold").and_then(Value::as_bool).unwrap_or(false);
                    let italic = f.get("italic").and_then(Value::as_bool).unwrap_or(false);
                    let mut text = String::new();
                    for c in node.get("content").and_then(Value::as_array).into_iter().flatten() {
                        match c.get("type").and_then(Value::as_str).unwrap_or("") {
                            "text" => text.push_str(c.get("text").and_then(Value::as_str).unwrap_or("")),
                            "tab" => text.push('\t'),
                            "break" => text.push_str("  \n"),
                            _ => {}
                        }
                    }
                    if text.is_empty() {
                        continue;
                    }
                    let (mut open, mut close) = (String::new(), String::new());
                    if bold {
                        open.push_str("**");
                        close.insert_str(0, "**");
                    }
                    if italic {
                        open.push('*');
                        close.insert(0, '*');
                    }
                    out.push_str(&open);
                    out.push_str(&text);
                    out.push_str(&close);
                }
                "hyperlink" => {
                    let mut text = String::new();
                    inline(node.get("children").or_else(|| node.get("content")).unwrap_or(&Value::Null), &mut text);
                    match find(node, "href").and_then(Value::as_str) {
                        Some(href) => out.push_str(&format!("[{text}]({href})")),
                        None => out.push_str(&text),
                    }
                }
                _ => {
                    if let Some(c) = node.get("content").filter(|c| c.is_array()) {
                        inline(c, out);
                    } else if let Some(c) = node.get("children").filter(|c| c.is_array()) {
                        inline(c, out);
                    }
                }
            }
        }
    }

    /// Adjacent runs with the same marks join as mammoth joins them.
    fn tidy(s: String) -> String {
        s.replace("****", "").replace("**", "\u{1}").replace('\u{1}', "**")
    }

    enum Block {
        Para(String),
        Heading(usize, String),
        Item(bool, String),
        Table(Vec<Vec<String>>),
    }

    fn blocks(ctx: &Ctx, content: &Value, out: &mut Vec<Block>) {
        for b in content.as_array().into_iter().flatten() {
            match b.get("type").and_then(Value::as_str).unwrap_or("") {
                "paragraph" => {
                    let mut text = String::new();
                    inline(b.get("content").unwrap_or(&Value::Null), &mut text);
                    let text = tidy(text);
                    if text.trim().is_empty() {
                        continue;
                    }
                    let f = b.get("formatting").cloned().unwrap_or(Value::Null);
                    let style = f.get("styleId").and_then(Value::as_str).unwrap_or("");
                    if let Some(n) = ctx.heading(style) {
                        out.push(Block::Heading(n, text));
                    } else if let Some(num) = find(&f, "numPr") {
                        let level = num.get("ilvl").and_then(Value::as_f64).unwrap_or(0.0) as u64;
                        out.push(Block::Item(ctx.ordered(num.get("numId").unwrap_or(&Value::Null), level), text));
                    } else {
                        out.push(Block::Para(text));
                    }
                }
                "table" => {
                    let mut rows = Vec::new();
                    for row in b.get("rows").and_then(Value::as_array).into_iter().flatten() {
                        let mut cells = Vec::new();
                        for cell in row.get("cells").and_then(Value::as_array).into_iter().flatten() {
                            let mut inner = Vec::new();
                            blocks(ctx, cell.get("content").unwrap_or(&Value::Null), &mut inner);
                            let text: Vec<String> = inner
                                .into_iter()
                                .map(|b| match b {
                                    Block::Para(t) | Block::Heading(_, t) | Block::Item(_, t) => t,
                                    Block::Table(_) => String::new(),
                                })
                                .collect();
                            cells.push(text.join("").trim().to_owned());
                        }
                        rows.push(cells);
                    }
                    out.push(Block::Table(rows));
                }
                "blockSdt" => blocks(ctx, b.get("content").unwrap_or(&Value::Null), out),
                _ => {}
            }
        }
    }

    /// mammoth's HTML through files.mjs's `htmlToMd`.
    pub fn docx(path: &Path) -> Result<String, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let doc = betteroffice_docx::Document::open(&bytes).map_err(|e| e.to_string())?;
        let model = doc.model();
        let styles_json = serde_json::to_value(&model.styles).unwrap_or(Value::Null);
        let styles =
            styles_json.get("styles").and_then(Value::as_array).into_iter().flatten().filter_map(|s| Some((s.get("styleId")?.as_str()?.to_owned(), s.get("name")?.as_str()?.to_owned()))).collect();
        let ctx = Ctx { styles, numbering: serde_json::to_value(&model.numbering).unwrap_or(Value::Null) };
        let body = serde_json::to_value(&model.body).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        blocks(&ctx, body.get("content").unwrap_or(&Value::Null), &mut out);
        let mut md = String::new();
        let mut i = 0;
        while i < out.len() {
            match &out[i] {
                Block::Para(t) => md.push_str(&format!("\n\n{}\n\n", lead(t))),
                Block::Heading(n, t) => md.push_str(&format!("\n\n{} {}\n\n", "#".repeat(*n), lead(t))),
                Block::Item(ordered, _) => {
                    let ordered = *ordered;
                    let mut items = Vec::new();
                    while let Some(Block::Item(o, t)) = out.get(i) {
                        if *o != ordered {
                            break;
                        }
                        items.push(t.clone());
                        i += 1;
                    }
                    let lines: Vec<String> = items.iter().enumerate().map(|(n, t)| if ordered { format!("{}. {}", n + 1, lead(t)) } else { format!("- {}", lead(t)) }).collect();
                    md.push_str(&format!("\n\n{}\n\n", lines.join("\n")));
                    continue;
                }
                Block::Table(rows) => {
                    if let Some(head) = rows.first() {
                        let sep: Vec<&str> = head.iter().map(|_| "---").collect();
                        let mut lines = vec![format!("| {} |", head.join(" | ")), format!("| {} |", sep.join(" | "))];
                        lines.extend(rows[1..].iter().map(|r| format!("| {} |", r.join(" | "))));
                        md.push_str(&format!("\n\n{}\n\n", lines.join("\n")));
                    }
                }
            }
            i += 1;
        }
        let collapsed = regex_lite::Regex::new(r"\n{3,}").map(|re| re.replace_all(&md, "\n\n").into_owned()).unwrap_or(md);
        Ok(collapsed.trim().to_owned())
    }

    /// `inline()`'s last step: a leading bullet glyph becomes `- `.
    fn lead(t: &str) -> String {
        let t2 = t.strip_prefix('\t').unwrap_or(t);
        match t2.strip_prefix("•\t") {
            Some(rest) => format!("- {rest}"),
            None => t.to_owned(),
        }
    }

    /// ExcelJS's view of each sheet: rows of cell strings (a formula's
    /// cached result, numbers as JavaScript prints them), bold flags, and
    /// column widths.
    pub fn sheets(path: &Path) -> Result<Value, String> {
        use betteroffice_xlsx::CellValue;
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let wb = betteroffice_xlsx::Workbook::open_for_read(&bytes).map_err(|e| e.to_string())?;
        let model = wb.model();
        let mut out = Vec::new();
        for sheet in &model.sheets {
            let (mut max_row, mut max_col) = (0usize, 0usize);
            let mut cells = Vec::new();
            for (at, cell) in sheet.iter_cells() {
                let (r, c) = (at.row as usize, at.col as usize);
                max_row = max_row.max(r + 1);
                max_col = max_col.max(c + 1);
                let text = match &cell.value {
                    CellValue::Number { value } => crate::js::number(*value),
                    CellValue::Text { value } => value.clone(),
                    CellValue::Bool { value } => value.to_string(),
                    _ => String::new(),
                };
                let bold =
                    sheet.effective_style(at).and_then(|xf| model.styles.cell_xfs.get(xf as usize)).and_then(|xf| xf.font).and_then(|f| model.styles.fonts.get(f as usize)).is_some_and(|f| f.bold);
                cells.push((r, c, text, bold));
            }
            for m in &sheet.merges {
                max_row = max_row.max(m.end.row as usize + 1);
                max_col = max_col.max(m.end.col as usize + 1);
            }
            let mut rows = vec![vec![String::new(); max_col]; max_row];
            let mut bold = vec![vec![false; max_col]; max_row];
            for (r, c, t, b) in cells {
                rows[r][c] = t;
                bold[r][c] = b;
            }
            // ExcelJS reads every cell of a merge as its master's value
            for m in &sheet.merges {
                let master = rows[m.start.row as usize][m.start.col as usize].clone();
                for r in m.start.row..=m.end.row {
                    for c in m.start.col..=m.end.col {
                        rows[r as usize][c as usize] = master.clone();
                    }
                }
            }
            let widths: Vec<Value> = (0..max_col).map(|c| sheet.col_widths.get(&(c as u32)).map_or(json!(0), |w| json!(w))).collect();
            rows.truncate(2000);
            bold.truncate(2000);
            out.push(json!({ "name": sheet.name, "rows": rows, "bold": bold, "widths": widths }));
        }
        Ok(Value::Array(out))
    }
}
