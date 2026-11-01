//! The model's view of a user turn (agent.mjs `userContent`,
//! `artifactBlocks`, `fileBody`): the quoted reply, the text, attached
//! images as image blocks, documents inlined, a `module://` card's source
//! (or its image).
//!
//! Plan D31: a workbook's or a Word file's text comes from Node's office
//! role (`GET /file/render`, through the front door), which reads only the
//! files dir; a deck's markdown export (`officeMarkdown`) has no route
//! there, so a deck, and a workbook or Word file outside the files dir,
//! reads as a binary file until 6.1.

use std::path::Path;

use base64::Engine;
use serde_json::{json, Map, Value};

use super::files::file_meta;
use crate::hub::ctx::HubCtx;
use crate::hub::pipeline::{versions, Dirs};
use crate::hub::tools::artifact::image_file_of;
use crate::hub::tools::office::out_file;
use crate::js;

/// `IMG_TYPE[ext]`.
pub fn img_type(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

/// files.mjs `parseDelimited(text, sep)`.
pub(crate) fn parse_delimited(text: &str, sep: char) -> Vec<Vec<String>> {
    let cs: Vec<char> = text.chars().collect();
    let (mut rows, mut row, mut cell, mut q) = (Vec::new(), Vec::new(), String::new(), false);
    let mut i = 0;
    while i < cs.len() {
        let ch = cs[i];
        if q {
            if ch == '"' && cs.get(i + 1) == Some(&'"') {
                cell.push('"');
                i += 1;
            } else if ch == '"' {
                q = false;
            } else {
                cell.push(ch);
            }
        } else if ch == '"' {
            q = true;
        } else if ch == sep {
            row.push(std::mem::take(&mut cell));
        } else if ch == '\n' || ch == '\r' {
            if ch == '\r' && cs.get(i + 1) == Some(&'\n') {
                i += 1;
            }
            row.push(std::mem::take(&mut cell));
            rows.push(std::mem::take(&mut row));
        } else {
            cell.push(ch);
        }
        i += 1;
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

/// Node's office role's `GET /file/render?path=<rel>` (D31).
async fn render(ctx: &HubCtx, p: &Path) -> Option<Value> {
    let files = ctx.store.gen_dir().join("files");
    let rel = p.strip_prefix(&files).ok()?.to_string_lossy().into_owned();
    let port = crate::hub::market::node_paths().1;
    let url = url::Url::parse(&format!("http://127.0.0.1:{port}/file/render?path={}", crate::hub::library::botctx::encode_uri_component(&rel))).ok()?;
    let r = crate::helper::http::send(&crate::helper::http::Req::get(url)).await.ok()?;
    serde_json::from_str(&r.text().await.ok()?).ok()
}

/// `fileBody(p, m)`: the file as text for the model, or `None` (binary).
pub async fn file_body(ctx: &HubCtx, p: &Path, m: &Map<String, Value>) -> Result<Option<String>, String> {
    let kind = m.get("kind").and_then(Value::as_str).unwrap_or("");
    let ext = m.get("ext").and_then(Value::as_str).unwrap_or("");
    let read = || std::fs::read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| e.to_string());
    if ["markdown", "text", "json", "html"].contains(&kind) || ext == "svg" {
        return read().map(Some);
    }
    let sheet_text = |name: &str, rows: &[Vec<String>]| format!("# sheet: {name}\n{}", rows.iter().map(|r| r.join("\t")).collect::<Vec<_>>().join("\n"));
    if kind == "table" {
        if ext == "csv" || ext == "tsv" {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let rows: Vec<Vec<String>> = parse_delimited(&read()?, if ext == "tsv" { '\t' } else { ',' }).into_iter().take(2000).collect();
            return Ok(Some(sheet_text(&name, &rows)));
        }
        let Some(r) = render(ctx, p).await else { return Ok(None) };
        let sheets = r.get("sheets").and_then(Value::as_array).cloned().unwrap_or_default();
        return Ok(Some(
            sheets
                .iter()
                .map(|s| {
                    let rows: Vec<Vec<String>> = s.get("rows").and_then(Value::as_array).into_iter().flatten().map(|r| r.as_array().into_iter().flatten().map(js::string_of).collect()).collect();
                    sheet_text(&s.get("name").map(js::string_of).unwrap_or_else(|| "undefined".into()), &rows)
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
        ));
    }
    if kind == "docx" {
        let Some(r) = render(ctx, p).await else { return Ok(None) };
        return Ok(Some(r.get("text").filter(|t| js::truthy(t)).map(js::string_of).unwrap_or_default()));
    }
    Ok(None)
}

fn image_block(media: &str, data: &[u8]) -> Value {
    json!({"type": "image", "source": {"type": "base64", "media_type": media, "data": base64::engine::general_purpose::STANDARD.encode(data)}})
}

/// A file inlined as text, or described when it can't be.
async fn text_block(ctx: &HubCtx, p: &Path, m: &Map<String, Value>, name: &str) -> Result<Value, String> {
    Ok(match file_body(ctx, p, m).await? {
        Some(body) => json!({"type": "text", "text": format!("<file name=\"{name}\">\n{}\n</file>", js::slice_utf16(&body, 60000))}),
        None => {
            let (kind, size) = (js::string_of(&m["kind"]), js::string_of(&m["size"]));
            json!({"type": "text", "text": format!("<file name=\"{name}\" kind=\"{kind}\" size=\"{size}\">(binary {kind} file; its contents can't be shown as text)</file>")})
        }
    })
}

/// `artifactBlocks(rels)`: a reply's files (at most 8) as context.
pub async fn artifact_blocks(ctx: &HubCtx, rels: Option<&Value>) -> Vec<Value> {
    let mut blocks = Vec::new();
    for rel in rels.and_then(Value::as_array).into_iter().flatten().take(8) {
        let Some(p) = out_file(ctx, &js::string_of(rel)).filter(|p| p.exists()) else { continue };
        let m = file_meta(ctx, &p, None).await;
        let name = js::string_of(&m["name"]);
        let ext = js::string_of(&m["ext"]);
        let got: Result<Vec<Value>, String> = async {
            if m["kind"] == "image" {
                if let Some(t) = img_type(&ext) {
                    let data = std::fs::read(&p).map_err(|e| e.to_string())?;
                    return Ok(vec![image_block(t, &data), json!({"type": "text", "text": format!("(the image above is {name}, {}×{})", crate::hub::store::json::template(m.get("width")), crate::hub::store::json::template(m.get("height")))})]);
                }
            }
            Ok(vec![text_block(ctx, &p, &m, &name).await?])
        }
        .await;
        if let Ok(b) = got {
            blocks.extend(b);
        }
    }
    blocks
}

/// `module://R<n>[@v<k>]` → (name, k).
fn module_ref(uri: &str) -> Option<(String, Option<String>)> {
    let rest = uri.strip_prefix("module://R")?;
    let (digits, v) = match rest.split_once("@v") {
        Some((d, v)) => (d, Some(v)),
        None => (rest, None),
    };
    let num = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (num(digits) && v.is_none_or(num)).then(|| (format!("R{digits}"), v.map(str::to_owned)))
}

/// `userContent(prompt, extra, ctx)`: the text alone, or blocks (the
/// reply's files first, then each attachment, then the text).
pub async fn user_content(ctx: &HubCtx, prompt: &str, extra: &Value, ctx_blocks: &[Value]) -> Value {
    let quote = match extra.get("reply").and_then(|r| r.get("text")).filter(|t| js::truthy(t)) {
        Some(t) => format!("> {}\n\n", js::string_of(t).replace('\n', "\n> ")),
        None => String::new(),
    };
    let text = format!("{quote}{}", if prompt.is_empty() { "(see the attachments)" } else { prompt });
    let files: Vec<&Value> = extra
        .get("attachments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|a| a.get("uri").and_then(Value::as_str).is_some_and(|u| u.starts_with("file://") || u.starts_with("module://")))
        .collect();
    if files.is_empty() && ctx_blocks.is_empty() {
        return json!(text);
    }
    let mut blocks: Vec<Value> = ctx_blocks.to_vec();
    let gen = ctx.store.gen_dir();
    let assets = gen.join("assets");
    for a in files {
        let uri = a["uri"].as_str().unwrap_or("");
        let mm = module_ref(uri);
        if let Some((name, v)) = &mm {
            let vn = v.as_ref().map(|v| v.parse::<f64>().unwrap_or(0.0)).filter(|v| *v != 0.0);
            if let Some(img) = image_file_of(ctx, name, vn) {
                if let Ok(data) = std::fs::read(assets.join(&img)) {
                    blocks.push(image_block("image/png", &data));
                    let at = v.as_ref().map(|v| format!(" v{v}")).unwrap_or_default();
                    blocks.push(json!({"type": "text", "text": format!("(the image above is image card {name}{at}. To change it, call generate_image with edit_of: \"{name}\" and a prompt that says only what to change.)")}));
                }
                continue;
            }
        }
        let path = match &mm {
            Some((name, Some(v))) => versions::ver_file(&Dirs::of(&ctx.data), name, &js::number(v.parse::<f64>().unwrap_or(0.0))),
            Some((name, None)) => [gen.clone(), ctx.store.widgets_dir()].into_iter().map(|d| d.join(format!("{name}.tsx"))).find(|p| p.exists()),
            None if uri.starts_with("module://") => None,
            None => js::decode_uri_component(&uri[7..]).ok().map(std::path::PathBuf::from),
        };
        let Some(path) = path else { continue };
        let got: Result<Value, String> = async {
            if a.get("kind").and_then(Value::as_str) == Some("image") {
                // vision caps at ~5 MB: a huge upload's feed derivative is
                // what the model sees
                let big = crate::hub::store::json::js_number(a.get("size").filter(|s| js::truthy(s))) > 4_500_000.0;
                let model_path = match a.get("feed").filter(|f| js::truthy(f)) {
                    Some(f) if big => {
                        let f = js::string_of(f);
                        std::path::PathBuf::from(js::decode_uri_component(crate::js::slice_utf16(&f, usize::MAX).get(7..).unwrap_or("")).map_err(|_| "URI malformed".to_owned())?)
                    }
                    _ => path.clone(),
                };
                let ext = model_path.to_string_lossy().rsplit('.').next().unwrap_or("").to_lowercase();
                let data = std::fs::read(&model_path).map_err(|e| e.to_string())?;
                return Ok(image_block(img_type(&ext).unwrap_or("image/png"), &data));
            }
            let name = a.get("name").map(js::string_of).unwrap_or_else(|| "undefined".into());
            let m = file_meta(ctx, &path, Some(&name)).await;
            text_block(ctx, &path, &m, &name).await
        }
        .await;
        if let Ok(b) = got {
            blocks.push(b);
        }
    }
    blocks.push(json!({"type": "text", "text": text}));
    Value::Array(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_content_reads_csv_and_module_refs_as_node_does() {
        assert_eq!(parse_delimited("a,\"b,\"\"c\"\"\"\r\nd", ','), vec![vec!["a".to_owned(), "b,\"c\"".to_owned()], vec!["d".to_owned()]]);
        assert_eq!(module_ref("module://R6@v2"), Some(("R6".into(), Some("2".into()))));
        assert_eq!(module_ref("module://R6"), Some(("R6".into(), None)));
        assert_eq!(module_ref("module://R6@v"), None);
        assert_eq!(module_ref("module://x"), None);
    }

    #[tokio::test]
    async fn user_content_is_the_text_alone_without_files() {
        let ctx = HubCtx::new(std::env::temp_dir().join("rox-uc-none"), crate::hub::traits::Clock::manual());
        assert_eq!(user_content(&ctx, "hi", &json!({}), &[]).await, json!("hi"));
        assert_eq!(user_content(&ctx, "", &json!({"reply": {"text": "a\nb"}}), &[]).await, json!("> a\n> b\n\n(see the attachments)"));
    }
}
