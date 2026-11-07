//! The read side of files (server/src/routes/files.mjs): a reply's files
//! under `generated/files` (with Range), their saved versions, attachments,
//! avatars and generated assets. Every path is resolved as Node's
//! `path.join` resolves it, and a reply's file must stay inside the files
//! folder.
//!
//! What's served here is content, not the broker's own page, and the broker's origin is what its
//! routes trust (`guard.rs`): anything that reaches the URL of a file that can run script, a page on
//! this Mac included, would get every chat. So nothing here is sniffed, and HTML and SVG come with
//! `sandbox`: an origin of no one's and, on these routes, no script. The viewer's HTML preview has
//! `/file/preview/`, the same files with `allow-scripts` added: script runs, at that empty origin,
//! where `guard.rs` turns it away. That goes for the preview's own files too: classic scripts,
//! stylesheets, images and media load, and what a browser fetches with CORS, `fetch` itself, module
//! scripts and fonts, doesn't. Letting `Origin: null` in here isn't safe: every sandboxed frame on
//! every website has that origin, and so has a planted page, and either could then read any file.

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
/// Beside [`RAW_TYPES`] on `/file/preview/`: what a page's own relative links load, which `nosniff`
/// would refuse as a download.
const PAGE_TYPES: &[(&str, &str)] = &[
    ("js", "text/javascript; charset=utf-8"),
    ("mjs", "text/javascript; charset=utf-8"),
    ("css", "text/css; charset=utf-8"),
    ("json", "application/json"),
    ("wasm", "application/wasm"),
    ("woff", "font/woff"),
    ("woff2", "font/woff2"),
    ("ttf", "font/ttf"),
    ("otf", "font/otf"),
    ("avif", "image/avif"),
    ("bmp", "image/bmp"),
    ("ico", "image/x-icon"),
    ("webm", "video/webm"),
    ("ogv", "video/ogg"),
    ("ogg", "audio/ogg"),
    ("flac", "audio/flac"),
];
/// Takes a file's document out of the broker's origin, and stops its script.
const NO_SCRIPT: &str = "sandbox";
/// The same, for a page a person opened to see it work.
const OWN_SCRIPT: &str = "sandbox allow-scripts";
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

/// Whether a browser shown `content_type` as a document runs what's in it.
fn runs_script(content_type: &str) -> bool {
    content_type.starts_with("text/html") || content_type.starts_with("image/svg")
}

/// `sendRanged`: the whole file, or the asked-for bytes (206), or 416.
fn send_ranged(path: &str, content_type: &str, range: Option<&str>, extra: Option<(&'static str, &str)>) -> Answer {
    send_as(path, content_type, range, extra, NO_SCRIPT)
}

/// `policy` goes on what [`runs_script`], whatever else differs: a part of a page is that page.
fn send_as(path: &str, content_type: &str, range: Option<&str>, extra: Option<(&'static str, &str)>, policy: &str) -> Answer {
    let with_extra = |a: Answer| {
        let a = match extra {
            Some((k, v)) => a.header(k, v),
            None => a,
        };
        let a = a.header("x-content-type-options", "nosniff");
        if runs_script(content_type) {
            a.header("content-security-policy", policy)
        } else {
            a
        }
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

/// `GET /file/preview/<rel>`: a reply's file for the viewer's own page, its script on, at no origin.
pub fn preview(leaf: &Leaf, url: &str, range: Option<&str>) -> Answer {
    let rest = url["/file/preview/".len()..].split('?').next().unwrap_or("");
    let Ok(rel) = js::decode_uri_component(rest) else { return thrown("URI malformed") };
    match leaf.out_file(&rel) {
        Some(p) if Path::new(&p).exists() => {
            let ext = ext_of(&p);
            let content_type = PAGE_TYPES.iter().find(|(e, _)| *e == ext).map_or_else(|| type_of(RAW_TYPES, &ext), |(_, t)| *t);
            send_as(&p, content_type, range, None, OWN_SCRIPT)
        }
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

    /// A folder no other test here has: they run in parallel, often in the same millisecond.
    fn scratch_dir() -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("buwiz-leaf-files-{}-{}-{n}", std::process::id(), crate::hub::store::now_ms()))
    }

    /// A broker's folders, with one file of each kind under each route.
    fn leaf_with(names: &[&str]) -> (Leaf, std::path::PathBuf) {
        let dir = scratch_dir();
        for folder in ["files/m1", "attachments", "avatars", "assets"] {
            std::fs::create_dir_all(dir.join("generated").join(folder)).unwrap();
            for name in names {
                std::fs::write(dir.join("generated").join(folder).join(name), "<script>x</script>").unwrap();
            }
        }
        (Leaf::new(dir.clone(), Vec::new()), dir)
    }

    fn header<'a>(a: &'a Answer, name: &str) -> Option<&'a str> {
        a.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    #[test]
    fn two_tests_never_share_a_folder() {
        let dirs: std::collections::HashSet<_> = (0..1000).map(|_| scratch_dir()).collect();
        assert_eq!(dirs.len(), 1000);
    }

    #[test]
    fn no_file_route_runs_script_at_the_brokers_origin() {
        let (leaf, dir) = leaf_with(&["a.html", "a.htm", "a.svg", "A.HTML"]);
        for name in ["a.html", "a.htm", "a.svg", "A.HTML"] {
            let a = raw(&leaf, &format!("/file/raw/m1/{name}"), None);
            assert_eq!(a.status, 200);
            assert_eq!(header(&a, "content-security-policy"), Some("sandbox"), "{name}");
            assert_eq!(header(&a, "x-content-type-options"), Some("nosniff"), "{name}");
            let part = raw(&leaf, &format!("/file/raw/m1/{name}"), Some("bytes=0-3"));
            assert_eq!(part.status, 206);
            assert_eq!(header(&part, "content-security-policy"), Some("sandbox"), "a range of it: {name}");
        }
        let a = asset(&leaf, "/asset/generated/assets/a.svg", None);
        assert_eq!((header(&a, "content-security-policy"), header(&a, "x-content-type-options")), (Some("sandbox"), Some("nosniff")));
        // no type that runs here at all, and nothing to sniff one from
        for url in ["/files/attachments/a.html", "/files/avatars/a.svg"] {
            let a = files_dir(&leaf, url, None);
            assert_eq!((header(&a, "content-type"), header(&a, "x-content-type-options")), (Some("application/octet-stream"), Some("nosniff")), "{url}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_named_as_something_else_isnt_sniffed() {
        let (leaf, dir) = leaf_with(&["a.png", "a.bin", "a.pdf"]);
        for url in ["/file/raw/m1/a.png", "/file/raw/m1/a.bin", "/file/raw/m1/a.pdf"] {
            let a = raw(&leaf, url, None);
            assert_eq!(header(&a, "x-content-type-options"), Some("nosniff"), "{url}");
            assert_eq!(header(&a, "content-security-policy"), None, "a sandbox stops a PDF's viewer: {url}");
        }
        for a in [files_dir(&leaf, "/files/attachments/a.png", None), files_dir(&leaf, "/files/avatars/a.png", None), asset(&leaf, "/asset/generated/assets/a.png", None)] {
            assert_eq!(header(&a, "x-content-type-options"), Some("nosniff"));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_preview_has_script_at_no_origin_and_what_a_page_links_to() {
        let (leaf, dir) = leaf_with(&["a.html", "a.svg", "a.js", "a.css", "a.png"]);
        for name in ["a.html", "a.svg"] {
            let a = preview(&leaf, &format!("/file/preview/m1/{name}"), None);
            assert_eq!(header(&a, "content-security-policy"), Some("sandbox allow-scripts"), "{name}");
            assert!(!header(&a, "content-security-policy").unwrap().contains("same-origin"));
        }
        assert_eq!(header(&preview(&leaf, "/file/preview/m1/a.js", None), "content-type"), Some("text/javascript; charset=utf-8"));
        assert_eq!(header(&preview(&leaf, "/file/preview/m1/a.css", None), "content-type"), Some("text/css; charset=utf-8"));
        assert_eq!(header(&preview(&leaf, "/file/preview/m1/a.png", None), "content-type"), Some("image/png"));
        assert_eq!(preview(&leaf, "/file/preview/m1/../../../etc", None).status, 404);
        for (name, kind) in [("a.wasm", "application/wasm"), ("a.woff2", "font/woff2"), ("a.ttf", "font/ttf"), ("a.avif", "image/avif"), ("a.webm", "video/webm"), ("a.ogg", "audio/ogg")] {
            std::fs::write(dir.join("generated/files/m1").join(name), "x").unwrap();
            assert_eq!(header(&preview(&leaf, &format!("/file/preview/m1/{name}"), None), "content-type"), Some(kind), "{name}");
        }
        // what runs script as a document stays what it was: not a type here
        std::fs::write(dir.join("generated/files/m1/a.xhtml"), "x").unwrap();
        assert_eq!(header(&preview(&leaf, "/file/preview/m1/a.xhtml", None), "content-type"), Some("application/octet-stream"));
        let _ = std::fs::remove_dir_all(dir);
    }

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
