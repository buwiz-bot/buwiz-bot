//! The read side of files (server/src/routes/files.mjs): a reply's files
//! under `generated/files` (with Range), their saved versions, attachments,
//! avatars and generated assets. Every path is resolved as Node's
//! `path.join` resolves it, and a reply's file must stay inside the files
//! folder.

use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use serde_json::{Map, Value};

use super::{Answer, Leaf};
use crate::js;

const RAW_TYPES: &[(&str, &str)] = &[
    ("pdf", "application/pdf"),
    ("html", "text/html; charset=utf-8"),
    ("htm", "text/html; charset=utf-8"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("svg", "image/svg+xml"),
    ("mp4", "video/mp4"),
    ("mov", "video/quicktime"),
    ("m4v", "video/mp4"),
    ("m4a", "audio/mp4"),
    ("mp3", "audio/mpeg"),
    ("wav", "audio/wav"),
    ("aac", "audio/aac"),
];
const MEDIA_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("mov", "video/quicktime"),
    ("mp4", "video/mp4"),
];
const ASSET_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("svg", "image/svg+xml"),
    ("mp4", "video/mp4"),
];

/// What Node's dispatcher sends when a route throws before setting a type.
fn thrown(error: &str) -> Answer {
    Answer::bare(500, format!("{{\"error\":{}}}", js::quote(error)))
}

fn not_found() -> Answer {
    Answer::bare(404, Vec::new())
}

/// Node's `path.join` / `path.normalize` (POSIX).
pub fn normalize(path: &str) -> String {
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

/// `path.join(dir, name)`: an empty part is dropped, not joined.
fn join(dir: &str, name: &str) -> String {
    if name.is_empty() {
        return normalize(dir);
    }
    normalize(&format!("{dir}/{name}"))
}

/// `(s.match(/\.([^.]+)$/)?.[1] || "").toLowerCase()`
fn ext_of(s: &str) -> String {
    s.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default()
}

fn type_of(table: &[(&str, &'static str)], ext: &str) -> &'static str {
    table.iter().find(|(e, _)| *e == ext).map(|(_, t)| *t).unwrap_or("application/octet-stream")
}

impl Leaf {
    fn gen_dir(&self) -> String {
        normalize(&format!("{}/generated", self.data.to_string_lossy()))
    }

    /// `outFile(rel)`: a path under the files folder, else `None`.
    fn out_file(&self, rel: &str) -> Option<String> {
        let out = format!("{}/files", self.gen_dir());
        let p = join(&out, rel);
        p.starts_with(&format!("{out}/")).then_some(p)
    }
}

/// `/bytes=(\d*)-(\d*)/` on the Range header: the two numbers as given.
fn byte_range(header: &str) -> Option<(&str, &str)> {
    let mut from = 0;
    while let Some(at) = header[from..].find("bytes=") {
        let rest = &header[from + at + 6..];
        let a = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if rest[a..].starts_with('-') {
            let tail = &rest[a + 1..];
            let b = tail.len() - tail.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            return Some((&rest[..a], &tail[..b]));
        }
        from += at + 6;
    }
    None
}

/// `sendRanged`: the whole file, or the asked-for bytes (206), or 416.
fn send_ranged(path: &str, content_type: &str, range: Option<&str>, extra: Option<(&'static str, &str)>) -> Answer {
    let with_extra = |a: Answer| match extra {
        Some((k, v)) => a.header(k, v),
        None => a,
    };
    let meta = match std::fs::metadata(path) {
        Ok(m) if m.is_dir() => return with_extra(thrown("EISDIR: illegal operation on a directory, read")),
        Ok(m) => m,
        Err(e) => return with_extra(thrown(&e.to_string())),
    };
    let len = meta.len() as f64;
    let read = |start: u64, count: u64| -> std::io::Result<Vec<u8>> {
        let mut f = std::fs::File::open(path)?;
        f.seek(SeekFrom::Start(start))?;
        let mut buf = Vec::with_capacity(count as usize);
        f.take(count).read_to_end(&mut buf)?;
        Ok(buf)
    };
    let ok = |status: u16, bytes: Vec<u8>| with_extra(Answer::bare(status, bytes)).header("content-type", content_type).header("accept-ranges", "bytes");
    let num = |s: &str| s.parse::<f64>().unwrap_or(f64::INFINITY);
    let Some((a, b)) = range.and_then(byte_range) else {
        return match read(0, meta.len()) {
            Ok(bytes) => ok(200, bytes),
            Err(e) => with_extra(thrown(&e.to_string())),
        };
    };
    let start = if a.is_empty() { (len - if b.is_empty() { 0.0 } else { num(b) }).max(0.0) } else { num(a) };
    let end = if !a.is_empty() && !b.is_empty() { num(b).min(len - 1.0) } else { len - 1.0 };
    if start >= len || start > end {
        return ok(416, Vec::new()).header("content-range", format!("bytes */{}", meta.len()));
    }
    match read(start as u64, (end - start) as u64 + 1) {
        Ok(bytes) => ok(206, bytes).header("content-range", format!("bytes {}-{}/{}", start as u64, end as u64, meta.len())),
        Err(e) => with_extra(thrown(&e.to_string())),
    }
}

/// `GET /file/raw/<rel>`: a reply's file.
pub fn raw(leaf: &Leaf, url: &str, range: Option<&str>) -> Answer {
    let rest = url["/file/raw/".len()..].split('?').next().unwrap_or("");
    let Ok(rel) = js::decode_uri_component(rest) else { return thrown("URI malformed") };
    match leaf.out_file(&rel) {
        Some(p) if Path::new(&p).exists() => send_ranged(&p, type_of(RAW_TYPES, &ext_of(&p)), range, None),
        _ => not_found(),
    }
}

/// `.versions/<rel>/v<n>.<ext>` → (rel, n), the regex case-insensitive.
fn version_of(path: &str) -> Option<String> {
    let rest = path.get(..10).filter(|p| p.eq_ignore_ascii_case(".versions/")).map(|_| &path[10..])?;
    let (rel, file) = rest.rsplit_once('/')?;
    let (stem, ext) = file.split_once('.')?;
    let digits = stem.strip_prefix(['v', 'V'])?;
    let ok = !rel.is_empty()
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && !ext.is_empty()
        && ext.bytes().all(|b| b.is_ascii_alphanumeric());
    ok.then(|| rel.to_owned())
}

/// `GET /file/versions?path=<rel>`: `{v, versions: [{v, at, bytes, path}]}`.
pub fn versions(leaf: &Leaf, url: &str) -> Answer {
    let param = js::search_param(url, "path").unwrap_or_default();
    let rel = version_of(&param).unwrap_or(param);
    let list = match versions_of_file(leaf, &rel) {
        Ok(list) => list,
        Err(e) => return Answer::thrown(&e),
    };
    let v = list.last().and_then(|e| e.get("v")).cloned().unwrap_or(Value::from(0));
    let mut m = Map::new();
    m.insert("v".into(), v);
    m.insert("versions".into(), Value::Array(list));
    Answer::json(200, js::stringify(&Value::Object(m)))
}

fn versions_of_file(leaf: &Leaf, rel: &str) -> Result<Vec<Value>, String> {
    let Some(dir) = leaf.out_file(&format!(".versions/{rel}")) else { return Ok(Vec::new()) };
    if !Path::new(&dir).exists() {
        return Ok(Vec::new());
    }
    // readdirSync lists names in byte order
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect();
    names.sort();
    let mut found: Vec<(f64, String)> = names
        .into_iter()
        .filter_map(|f| {
            let digits: String = f.strip_prefix('v')?.chars().take_while(char::is_ascii_digit).collect();
            let dot = f.as_bytes().get(1 + digits.len()) == Some(&b'.');
            (!digits.is_empty() && dot).then(|| (digits.parse::<f64>().unwrap_or(f64::INFINITY), f))
        })
        .collect();
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    found
        .into_iter()
        .map(|(v, f)| {
            let st = std::fs::metadata(format!("{dir}/{f}")).map_err(|e| e.to_string())?;
            let mtime_ms = st.mtime() as f64 * 1000.0 + st.mtime_nsec() as f64 / 1e6;
            let mut m = Map::new();
            m.insert("v".into(), Value::from(v));
            m.insert("at".into(), Value::from((mtime_ms + 0.5).floor()));
            m.insert("bytes".into(), Value::from(st.len()));
            m.insert("path".into(), Value::String(format!(".versions/{rel}/{f}")));
            Ok(Value::Object(m))
        })
        .collect()
}

/// `GET /files/(attachments|avatars)/<name>`: by bare file name.
pub fn files_dir(leaf: &Leaf, url: &str, range: Option<&str>) -> Answer {
    let (dir, rest) = match url.strip_prefix("/files/attachments/") {
        Some(rest) => ("attachments", rest),
        None => ("avatars", url.strip_prefix("/files/avatars/").unwrap_or("")),
    };
    let raw = rest.split('?').next().unwrap_or("");
    if raw.is_empty() {
        return thrown("object null is not iterable (cannot read property Symbol(Symbol.iterator))");
    }
    let Ok(name) = js::decode_uri_component(raw) else { return thrown("URI malformed") };
    let path = join(&format!("{}/{dir}", leaf.gen_dir()), name.rsplit('/').next().unwrap_or(""));
    if !Path::new(&path).exists() {
        return not_found();
    }
    send_ranged(&path, type_of(MEDIA_TYPES, &ext_of(&path)), range, Some(("cache-control", "max-age=86400")))
}

/// `GET /asset/generated/assets/<file>`: a mini app's generated asset.
pub fn asset(leaf: &Leaf, url: &str, range: Option<&str>) -> Answer {
    let rest = url["/asset/generated/assets/".len()..].split('?').next().unwrap_or("");
    let Ok(decoded) = js::decode_uri_component(rest) else { return thrown("URI malformed") };
    let name = decoded.rsplit('/').next().unwrap_or("");
    let path = join(&format!("{}/assets", leaf.gen_dir()), name);
    if name.is_empty() || !Path::new(&path).exists() {
        return not_found();
    }
    send_ranged(&path, type_of(ASSET_TYPES, &ext_of(name)), range, Some(("cache-control", "max-age=86400")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_join_as_node_joins_them() {
        assert_eq!(normalize("/d/generated/files/t1/a.pdf"), "/d/generated/files/t1/a.pdf");
        assert_eq!(normalize("/d/generated/files/../app.db"), "/d/generated/app.db");
        assert_eq!(normalize("/d//x/./y/"), "/d/x/y/");
        assert_eq!(normalize("/../../a"), "/a");
        assert_eq!(normalize("a/../../b"), "../b");
        assert_eq!(normalize("./"), "./");
        assert_eq!(normalize("a/../"), "./");
        assert_eq!(normalize("//"), "/");
        assert_eq!(normalize(""), ".");
        let leaf = Leaf::new("/d/".into(), Vec::new());
        assert_eq!(leaf.out_file("t1/a.pdf").as_deref(), Some("/d/generated/files/t1/a.pdf"));
        assert_eq!(leaf.out_file("../app.db"), None);
        assert_eq!(leaf.out_file(""), None);
        assert_eq!(leaf.out_file("x/.."), None);
    }

    #[test]
    fn ranges_read_as_node_reads_them() {
        assert_eq!(byte_range("bytes=2-5"), Some(("2", "5")));
        assert_eq!(byte_range("bytes=-"), Some(("", "")));
        assert_eq!(byte_range("x bytes=a, bytes=7-"), Some(("7", "")));
        assert_eq!(byte_range("items=1-2"), None);
    }

    #[test]
    fn a_version_path_names_its_file() {
        assert_eq!(version_of(".versions/t1/a.docx/v2.docx").as_deref(), Some("t1/a.docx"));
        assert_eq!(version_of(".VERSIONS/t1/a.docx/V10.DOCX").as_deref(), Some("t1/a.docx"));
        assert_eq!(version_of(".versions/v2.docx"), None);
        assert_eq!(version_of("t1/a.docx"), None);
        assert_eq!(version_of(".versions/t1/v2.do-cx"), None);
        assert_eq!(ext_of("/a.b/c"), "b/c");
        assert_eq!(ext_of("noext"), "");
    }
}
