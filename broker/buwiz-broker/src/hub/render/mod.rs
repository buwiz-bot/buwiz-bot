//! The last routes Node's office role answered (plan phase 7), so the hub
//! runs with no Node at all:
//!
//! - `GET /file/render?path=<rel>` for every kind but the office ones
//!   (routes/files.mjs and files.mjs `renderFile`): markdown, code and
//!   text (lines from `highlight`), JSON, HTML, CSV/TSV, `.xls`, and the
//!   meta of images, video, audio, PDFs and the rest; `app:R<n>` and
//!   `app:R<n>@v<k>` are a mini app's source. docx, xlsx and pptx go to
//!   `office::routes` (6.1).
//! - `GET /` (routes/misc.mjs): `app-server ok — <n> threads, model=<m>`.

pub mod highlight;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::{Response, StatusCode};
use serde_json::{json, Map, Value};

use crate::hub::agent::files::file_meta;
use crate::hub::ctx::HubCtx;
use crate::hub::pipeline::versions::head_of;
use crate::hub::types::{json as json_response, HubRequest, HubResponse, Pattern, Verb};
use crate::hub::Registry;
use crate::js;

const MAX_TEXT: usize = 400_000;

pub fn install(routes: &mut Registry) {
    routes.route(Verb::Get, Pattern::Exact("/file/render"), render);
    routes.route(Verb::Get, Pattern::Exact("/"), root);
}

fn text(status: StatusCode, body: String, content_type: Option<&'static str>) -> HubResponse {
    let mut res = Response::new(Full::new(Bytes::from(body)).map_err(|never| match never {}).boxed());
    *res.status_mut() = status;
    if let Some(t) = content_type {
        res.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static(t));
    }
    res
}

/// Node's last resort for a URL no route takes.
fn not_found() -> HubResponse {
    text(StatusCode::NOT_FOUND, "not found".to_owned(), None)
}

/// `GET /` (Node matches the URL exactly, so `/?x` is its 404).
async fn root(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    if req.url != "/" {
        return not_found();
    }
    let threads = ctx.store.read(|s| s.list.len());
    let settings = crate::hub::model::Settings::global();
    let model = if settings.mock { "mock" } else { settings.model.as_str() };
    text(StatusCode::OK, format!("app-server ok — {threads} threads, model={model}\n"), Some("text/plain"))
}

async fn render(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    // Node's route is `/file/render?…`: the bare path is its 404
    if !req.url.starts_with("/file/render?") {
        return not_found();
    }
    if crate::office::routes::renders(&req.url) {
        return crate::office::routes::render(ctx, req).await;
    }
    let rel = req.query("path").unwrap_or_default();
    let Some(p) = viewer_file(&ctx, &rel) else {
        return json_response(StatusCode::OK, &json!({ "error": "bad path" }));
    };
    match render_file(&ctx, &p).await {
        Ok(v) => json_response(StatusCode::OK, &Value::Object(v)),
        Err(e) => json_response(StatusCode::OK, &json!({ "error": e })),
    }
}

/// routes/files.mjs `viewerFile(path)`: a reply's file, or a mini app's
/// source (`app:R<n>`: the head version's file, else the module, else the
/// widget; `app:R<n>@v<k>`: that version's file).
fn viewer_file(ctx: &HubCtx, rel: &str) -> Option<PathBuf> {
    let Some(rest) = rel.strip_prefix("app:R") else {
        return crate::hub::library::artifacts::out_file(ctx, rel);
    };
    let (n, v) = match rest.split_once("@v") {
        Some((n, v)) => (n, Some(v)),
        None => (rest, None),
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(n) || !v.is_none_or(digits) {
        return crate::hub::library::artifacts::out_file(ctx, rel);
    }
    let name = format!("R{n}");
    let dirs = crate::hub::pipeline::Dirs::of(&ctx.data);
    if let Some(v) = v {
        // Number(k), printed as JavaScript prints it
        return crate::hub::pipeline::versions::ver_file(&dirs, &name, &js::number(v.parse::<f64>().unwrap_or(f64::NAN)));
    }
    let head = head_of(ctx, &name).ok().flatten().filter(|v| *v != 0).and_then(|v| crate::hub::pipeline::versions::ver_file(&dirs, &name, &v.to_string()));
    head.into_iter().chain([dirs.gen.join(format!("{name}.tsx")), dirs.widgets.join(format!("{name}.tsx"))]).find(|p| p.exists())
}

/// `readFileSync(p, "utf8")`, failing with Node's message.
fn read_text(p: &Path) -> Result<String, String> {
    match std::fs::read(p) {
        Ok(b) => Ok(String::from_utf8_lossy(&b).into_owned()),
        Err(e) => Err(match e.raw_os_error() {
            Some(libc::EISDIR) => "EISDIR: illegal operation on a directory, read".to_owned(),
            Some(libc::EACCES) => format!("EACCES: permission denied, open '{}'", p.display()),
            _ => e.to_string(),
        }),
    }
}

/// files.mjs `renderFile(path, rel)` for the kinds that aren't office's.
async fn render_file(ctx: &HubCtx, p: &Path) -> Result<Map<String, Value>, String> {
    let base = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // the name without an upload's `<id>-<4>-` prefix
    let name = regex_lite::Regex::new(r"^[a-z0-9]+-[a-z0-9]{4}-").map(|re| re.replace(&base, "").into_owned()).unwrap_or_else(|_| base.clone());
    let mut meta = file_meta(ctx, p, Some(&name)).await;
    let kind = js::string_of(&meta["kind"]);
    let ext = js::string_of(&meta["ext"]);
    if !p.exists() {
        meta.insert("error".into(), json!("missing"));
        return Ok(meta);
    }
    let p = p.to_path_buf();
    let content: Option<(&str, Value)> = match kind.as_str() {
        "markdown" => Some(("text", json!(js::slice_utf16(&read_text(&p)?, MAX_TEXT)))),
        "text" => {
            let text = js::slice_utf16(&read_text(&p)?, MAX_TEXT);
            Some(("lines", blocking(move || highlight::highlight(&text, &ext)).await?))
        }
        "json" => {
            let text = read_text(&p)?;
            match serde_json::from_str::<Value>(&text) {
                Ok(v) => Some(("json", v)),
                Err(_) => {
                    let text = js::slice_utf16(&text, MAX_TEXT);
                    Some(("lines", blocking(move || highlight::highlight(&text, "json")).await?))
                }
            }
        }
        "html" => {
            let text = js::slice_utf16(&read_text(&p)?, MAX_TEXT);
            Some(("lines", blocking(move || highlight::highlight(&text, "html")).await?))
        }
        "table" if ext == "csv" || ext == "tsv" => {
            let sep = if ext == "tsv" { '\t' } else { ',' };
            let rows: Vec<Value> = crate::hub::agent::user_content::parse_delimited(&read_text(&p)?, sep).into_iter().take(2000).map(|r| json!(r)).collect();
            Some(("sheets", json!([{ "name": base, "rows": rows }])))
        }
        // `.xls`: ExcelJS reads it as a zip (an xlsx by another name)
        "table" => Some(("sheets", blocking(move || xls_sheets(&p)).await??)),
        _ => None,
    };
    if let Some((key, value)) = content {
        meta.insert(key.into(), value);
    }
    Ok(meta)
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.map_err(|e| e.to_string())
}

/// ExcelJS's `xlsx.readFile` on a file that isn't `.xlsx`: JSZip's
/// refusals for what isn't a zip, else the workbook's sheets.
fn xls_sheets(p: &Path) -> Result<Value, String> {
    let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
    if bytes.len() < 4 {
        return Err(format!("End of data reached (data length = {}, asked index = 4). Corrupted zip ?", bytes.len()));
    }
    if !bytes.windows(4).any(|w| w == b"PK\x05\x06") {
        return Err("Can't find end of central directory : is this a zip file ? If it is, see https://stuk.github.io/jszip/documentation/howto/read_zip.html".to_owned());
    }
    crate::office::routes::markdown::sheets(p)
}
