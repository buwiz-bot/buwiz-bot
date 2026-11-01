//! The model-free slash commands of `generate` (agent.mjs): `/mock …`
//! (sample files of every kind, one kind, or `images n [ar]` numbered
//! tiles) and `/upload` (the message's attachments echoed back as file
//! cards).

use std::path::Path;

use serde_json::{json, Value};

use super::files::{file_record, mock_docx, mock_files, mock_images};
use crate::hub::store::Turn;
use crate::hub::tools::Run;
use crate::js;

/// `/^\/<cmd>\b/i` on the trimmed prompt.
pub fn slash(prompt: &str, cmd: &str) -> bool {
    let n = cmd.len() + 1;
    prompt.len() >= n && prompt.is_char_boundary(n) && prompt[..n].eq_ignore_ascii_case(&format!("/{cmd}")) && !prompt[n..].chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn ws(s: &str) -> usize {
    s.len() - s.trim_start_matches(js::is_space).len()
}

/// `\d+(?:\.\d+)?` at the start: its text.
fn number(s: &str) -> Option<&str> {
    let n = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if n == 0 {
        return None;
    }
    let rest = &s[n..];
    if let Some(frac) = rest.strip_prefix('.') {
        let m = frac.find(|c: char| !c.is_ascii_digit()).unwrap_or(frac.len());
        if m > 0 {
            return Some(&s[..n + 1 + m]);
        }
    }
    Some(&s[..n])
}

/// `/^\/mock\s+images?\s+(\d+)(?:\s+(A)\s*[x:/]\s*(B)|\s+(C))?/i`: the
/// count and the aspect.
pub fn images_of(p: &str) -> Option<(f64, Option<(f64, f64)>)> {
    let rest = p.get(5..).filter(|_| p[..5].eq_ignore_ascii_case("/mock"))?;
    let w = ws(rest);
    if w == 0 {
        return None;
    }
    let rest = &rest[w..];
    if rest.len() < 5 || !rest.is_char_boundary(5) || !rest[..5].eq_ignore_ascii_case("image") {
        return None;
    }
    let rest = &rest[5..];
    let rest = rest.strip_prefix(['s', 'S']).unwrap_or(rest);
    let w = ws(rest);
    let rest = &rest[w..];
    let n = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    if w == 0 || n == 0 {
        return None;
    }
    let count: f64 = rest[..n].parse().unwrap_or(f64::INFINITY);
    let tail = &rest[n..];
    let w = ws(tail);
    let ar = (|| {
        if w == 0 {
            return None;
        }
        let t = &tail[w..];
        let a = number(t)?;
        // A \s* [x:/] \s* B (a shorter A never helps: a digit or a dot
        // would follow it), else A alone
        let after = &t[a.len()..];
        let after = &after[ws(after)..];
        if let Some(sep) = after.chars().next().filter(|c| matches!(c, 'x' | 'X' | ':' | '/')) {
            let b_at = &after[sep.len_utf8()..];
            if let Some(b) = number(&b_at[ws(b_at)..]) {
                return Some((a.parse().unwrap_or(0.0), Some(b.parse().unwrap_or(0.0))));
            }
        }
        Some((a.parse().unwrap_or(0.0), None))
    })();
    let ar = ar.map(|(a, b)| match b {
        Some(b) => (a, b),
        None => (a, 1.0),
    });
    Some((count, ar))
}

/// `/^\/mock\s+([a-z0-9]+)\s*$/i`, lower-cased.
pub fn word_of(p: &str) -> Option<String> {
    let rest = p.get(5..).filter(|_| p[..5].eq_ignore_ascii_case("/mock"))?;
    let w = ws(rest);
    if w == 0 {
        return None;
    }
    let word = rest[w..].trim_end_matches(js::is_space);
    (!word.is_empty() && word.bytes().all(|b| b.is_ascii_alphanumeric())).then(|| word.to_ascii_lowercase())
}

fn alias(w: &str) -> String {
    match w {
        "markdown" => "md",
        "text" => "txt",
        "rust" => "rs",
        "vcard" => "vcf",
        "doc" | "document" | "word" => "docx",
        "xls" | "sheet" | "excel" | "spreadsheet" => "xlsx",
        "ppt" | "deck" | "powerpoint" | "presentation" | "slides" => "pptx",
        "video" => "mov",
        "image" | "images" => "png",
        other => other,
    }
    .to_owned()
}

const KNOWN: [&str; 20] = ["pdf", "md", "html", "txt", "json", "csv", "rs", "tsx", "png", "svg", "gif", "webp", "vcf", "mov", "m4a", "wav", "aiff", "docx", "xlsx", "pptx"];

fn office_seed(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "docx" => "betteroffice-demo.docx",
        "xlsx" => "showcase.xlsx",
        "pptx" => "presentation.pptx",
        _ => return None,
    })
}

/// `emitOffice(ext, paths)`: the BetterOffice demo file, else the
/// generated fallback.
fn emit_office(src: &Path, dir: &Path, ext: &str, paths: &mut Vec<std::path::PathBuf>) -> Result<(), String> {
    let seed = office_seed(ext).expect("an office ext");
    let s = src.join("office").join("seeds").join(seed);
    if s.exists() {
        let p = dir.join(seed);
        std::fs::copy(&s, &p).map_err(|e| e.to_string())?;
        paths.push(p);
        return Ok(());
    }
    match ext {
        "docx" => paths.push(mock_docx(dir).map_err(|e| e.to_string())?),
        // BUG: agent.mjs calls mockXlsx and mockPptx without importing them,
        // so a missing seed crashes the job
        "xlsx" => return Err("mockXlsx is not defined".into()),
        _ => return Err("mockPptx is not defined".into()),
    }
    Ok(())
}

/// `emitMovs(paths)`: a screen recording as the video sample, twice.
fn emit_movs(attach: &Path, dir: &Path, paths: &mut Vec<std::path::PathBuf>) -> Result<(), String> {
    let mut movs: Vec<String> =
        std::fs::read_dir(attach).map_err(|e| e.to_string())?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|f| f.ends_with(".mov")).collect();
    movs.sort();
    if let Some(m) = movs.first() {
        for n in ["demo.mov", "replay.mov"] {
            std::fs::copy(attach.join(m), dir.join(n)).map_err(|e| e.to_string())?;
        }
        paths.push(dir.join("demo.mov"));
        paths.push(dir.join("replay.mov"));
    }
    Ok(())
}

/// The user's turn and the reply, the title set on an empty thread, the
/// job done.
fn land(run: &Run, title: Option<&str>, user: Turn, ai: Turn, detail: &str) -> Result<(), String> {
    run.ctx.store.write(|s| {
        let Some(t) = s.thread_mut(&run.turn.thread) else { return };
        if let Some(title) = title {
            if t.turns.is_empty() {
                t.title = title.into();
            }
        }
        t.turns.push(user);
        t.turns.push(ai);
    })?;
    run.ctx.store.write_index();
    run.ctx.jobs.finish(&run.ctx.store, run.job, detail, Some("reply"));
    Ok(())
}

fn names(files: &[Value]) -> String {
    files.iter().map(|f| js::string_of(&f["name"])).collect::<Vec<_>>().join(", ")
}

/// `/mock …`.
pub async fn mock(run: &Run, prompt: &str, extra: &Value) -> Result<(), String> {
    let ctx = &run.ctx;
    let p = prompt.trim_matches(js::is_space);
    let rel = format!("m{}", crate::hub::library::groups::now36());
    let files_dir = ctx.store.gen_dir().join("files");
    let dir = files_dir.join(&rel);
    let (src, port) = crate::hub::market::node_paths();
    let user = || Turn::user(prompt).with_extra(extra);
    let record = |p: std::path::PathBuf| {
        let rel = rel.clone();
        async move {
            let key = format!("{rel}/{}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            file_record(ctx, &p, &key).await
        }
    };
    if let Some((n, ar)) = images_of(p) {
        let n = if n.is_nan() || n == 0.0 { 1.0 } else { n }.clamp(1.0, 24.0) as usize;
        let paths = mock_images(&dir, n, ar).map_err(|e| e.to_string())?;
        let mut files = Vec::new();
        for path in paths {
            files.push(record(path).await);
        }
        let enc = |p: &str| p.split('/').map(crate::hub::library::botctx::encode_uri_component).collect::<Vec<_>>().join("/");
        let attachments: Vec<Value> = files
            .iter()
            .map(|f| {
                let path = js::string_of(&f["path"]);
                let thumb = f.get("feed").filter(|x| js::truthy(x)).map(js::string_of).unwrap_or_else(|| path.clone());
                json!({
                    "uri": format!("http://127.0.0.1:{port}/file/raw/{}", enc(&path)),
                    "thumb": format!("http://127.0.0.1:{port}/file/raw/{}", enc(&thumb)),
                    "name": f.get("name"), "ext": f.get("ext"), "kind": "image", "size": f.get("size"), "width": f.get("width"), "height": f.get("height"),
                })
            })
            .collect();
        let sizes: Vec<String> = files.iter().map(|f| format!("{}×{}", crate::hub::store::json::template(f.get("width")), crate::hub::store::json::template(f.get("height")))).take(6).collect();
        let shape = match ar {
            Some((a, b)) => format!(" at {}:{}", js::number(a), js::number(b)),
            None => " across cycled aspects".into(),
        };
        let text = format!("{n} mock image{}{shape} — sized {}{}.", if n == 1 { "" } else { "s" }, sizes.join(", "), if n > 6 { ", …" } else { "" });
        let mut u = user();
        u.attachments = Some(Value::Array(attachments));
        let ai = Turn { files: Some(Value::Array(files)), ..Turn::ai(&text, "mock") };
        return land(run, Some(&format!("Mock images ×{n}")), u, ai, &format!("{n} images"));
    }
    let attach = ctx.store.gen_dir().join("attachments");
    if let Some(word) = word_of(p).filter(|w| w != "files" && w != "media") {
        let exts: Vec<String> = if word == "audio" { vec!["m4a".into(), "wav".into(), "aiff".into()] } else { vec![alias(&word)] };
        let mut paths = Vec::new();
        let text = if exts.iter().any(|e| !KNOWN.contains(&e.as_str())) {
            let mut sorted = KNOWN.to_vec();
            sorted.sort();
            format!("No mock for `{word}` — try one of: {} (or `files`, `media`, `images n`, `audio`).", sorted.join(", "))
        } else {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            for e in &exts {
                if office_seed(e).is_some() {
                    emit_office(&src, &dir, e, &mut paths)?;
                } else if e == "mov" {
                    emit_movs(&attach, &dir, &mut paths)?;
                }
            }
            let rest: Vec<String> = exts.iter().filter(|e| office_seed(e).is_none() && *e != "mov").cloned().collect();
            paths.extend(mock_files(ctx, &src, &dir, Some(&rest)).await);
            if paths.is_empty() {
                format!("Couldn't produce a {word} mock here — the tools that kind needs aren't on this machine.")
            } else {
                format!("Mock {word}: {}.", paths.iter().map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect::<Vec<_>>().join(", "))
            }
        };
        let mut files = Vec::new();
        for path in paths {
            files.push(record(path).await);
        }
        // `Mock <word>` first, then reply()'s own "Mock files"
        let ai = Turn { files: Some(Value::Array(files)), ..Turn::ai(&text, "mock") };
        return land(run, Some("Mock files"), user(), ai, "replied");
    }
    let mut paths = mock_files(ctx, &src, &dir, None).await;
    for ext in ["docx", "xlsx", "pptx"] {
        emit_office(&src, &dir, ext, &mut paths)?;
    }
    emit_movs(&attach, &dir, &mut paths)?;
    let mut files = Vec::new();
    for path in paths {
        files.push(record(path).await);
    }
    // an image whose file is gone: the gallery shows "Image unavailable"
    let first = files.iter().position(|f| f["kind"] == "image").map(|i| i + 1).unwrap_or(0);
    files.insert(first, json!({"path": format!("{rel}/missing.png"), "name": "missing.png", "ext": "png", "size": 0, "kind": "image", "variant": "media"}));
    if prompt.to_lowercase().contains("media") {
        files.retain(|f| f["kind"] == "image" || f["kind"] == "video");
    }
    let text =
        format!("Here are {} sample files — one of each kind the viewer handles.\n\n```mermaid\nflowchart LR\n  A[You ask] --> B[Kiffy makes the file] --> C[You click to view]\n```", files.len());
    let ai = Turn { files: Some(Value::Array(files)), ..Turn::ai(&text, "mock") };
    land(run, Some("Mock files"), user(), ai, "replied")
}

/// `/upload`: the attachments back as file cards, no model call.
pub async fn upload(run: &Run, prompt: &str, extra: &Value) -> Result<(), String> {
    let ctx = &run.ctx;
    let rel = format!("m{}", crate::hub::library::groups::now36());
    let dir = ctx.store.gen_dir().join("files").join(&rel);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut taken: Vec<String> = Vec::new();
    let mut files = Vec::new();
    for a in extra.get("attachments").and_then(Value::as_array).into_iter().flatten() {
        let uri = a.get("uri").and_then(Value::as_str).unwrap_or("");
        let Some(raw) = uri.strip_prefix("file://") else { continue };
        let srcp = js::decode_uri_component(raw).map_err(|_| "URI malformed".to_owned())?;
        if !Path::new(&srcp).exists() {
            continue;
        }
        let given = a.get("name").filter(|n| js::truthy(n)).map(js::string_of).unwrap_or_else(|| srcp.clone());
        let base = given.trim_end_matches('/').rsplit('/').next().unwrap_or("").replace('\\', "_");
        let unique = if taken.contains(&base) { format!("{}-{base}", taken.len()) } else { base };
        if !taken.contains(&unique) {
            taken.push(unique.clone());
        }
        let p = dir.join(&unique);
        std::fs::copy(&srcp, &p).map_err(|e| e.to_string())?;
        files.push(file_record(ctx, &p, &format!("{rel}/{unique}")).await);
    }
    let text = if files.is_empty() {
        "Attach a file and /upload echoes it back as a viewable card — nothing goes to the model.".to_owned()
    } else {
        format!("Echoed back {} — nothing went to the model: {}.", if files.len() == 1 { "your file".to_owned() } else { format!("{} files", files.len()) }, names(&files))
    };
    let ai = Turn { files: Some(Value::Array(files)), ..Turn::ai(&text, "upload") };
    land(run, Some("Upload echo"), Turn::user(prompt).with_extra(extra), ai, "replied")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_mock_reads_its_commands_as_the_regexes_do() {
        assert_eq!(images_of("/mock images 4"), Some((4.0, None)));
        assert_eq!(images_of("/MOCK image 2 16x9"), Some((2.0, Some((16.0, 9.0)))));
        assert_eq!(images_of("/mock images 3 4 : 3"), Some((3.0, Some((4.0, 3.0)))));
        assert_eq!(images_of("/mock images 3 1.78"), Some((3.0, Some((1.78, 1.0)))));
        assert_eq!(images_of("/mock images 3 16/9.5"), Some((3.0, Some((16.0, 9.5)))));
        assert_eq!(images_of("/mock images"), None);
        assert_eq!(images_of("/mock imagesx 3"), None);
        assert_eq!(word_of("/mock  PDF "), Some("pdf".into()));
        assert_eq!(word_of("/mock a b"), None);
        assert!(slash("/upload", "upload") && slash("/uploadx", "uploadx") && !slash("/uploads", "upload") && slash("/mock-x", "mock"));
    }
}
