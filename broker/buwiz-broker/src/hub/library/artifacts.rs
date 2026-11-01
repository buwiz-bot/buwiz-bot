//! The Library's one record per artifact (artifacts.mjs). Mini apps and
//! image cards (id `R<n>`), the files a reply made (`file:<path>`), the
//! Drive files it made or changed (`gdrive:<file id>`, `drive`) and the `@`
//! tokens attached to a chat (`attach:<id>`) share a shape:
//!   { id, kind, title, filename, description, icon, meta, origin, storage,
//!     versioning, versions, v, mtime, published }
//! Apps keep their details in `apps` (`appmeta`); images and files keep the
//! user's edits in `artifact_meta`; `library_hidden` archives any of them.
//!
//! Also here, for the tool loop (5.4): `image_file_of`, `image_module` (an
//! image card's module), `mcp_app_module` (`GET /module?name=mcp:…`).

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::types::Value as SqlValue;
use rusqlite::{params_from_iter, OptionalExtension};
use serde_json::{json, Map, Value};

use super::appmeta::{adopt_icon, app_info, app_summary, born_ms, gen_dir, hints, is_r_name, js_round, mtime_ms, num, row_of, rows_of, str_or_empty, trim_js, update_app_meta, Hint};
use super::drive::{drive_type, drive_write, DRIVE_SERVER};
use crate::hub::ctx::HubCtx;
use crate::hub::library::attachments;
use crate::hub::pipeline::versions::{head_of, ver_file, versions_of};
use crate::hub::pipeline::Dirs;
use crate::hub::store::json::js_slice;
use crate::hub::store::{now_ms, Thread, Turn};
use crate::hub::types::BoxFuture;
use crate::js;

// ————— files —————

/// files.mjs `extOf(name)`.
pub fn ext_of(name: &str) -> String {
    let n = name.rsplit(['/', '\\']).next().unwrap_or("");
    match n.rfind('.') {
        Some(i) if i > 0 && i + 1 < n.len() => n[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

const IMAGE: [&str; 11] = ["png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "avif", "bmp", "tiff", "svg"];
const VIDEO: [&str; 6] = ["mp4", "mov", "webm", "m4v", "mkv", "avi"];
const AUDIO: [&str; 9] = ["mp3", "wav", "m4a", "aac", "flac", "ogg", "oga", "opus", "aiff"];
const TABLE: [&str; 4] = ["csv", "tsv", "xlsx", "xls"];
const TEXTUAL: &[&str] = &[
    "md", "markdown", "mdx", "txt", "text", "rtf", "log", "html", "htm", "xml", "yaml", "yml", "toml", "ini", "cfg", "env", "sh", "bash", "zsh", "js", "jsx", "mjs", "cjs", "ts", "tsx", "py", "rb",
    "go", "rs", "java", "kt", "swift", "c", "h", "cc", "cpp", "hpp", "cs", "php", "sql", "css", "scss", "tex", "json", "jsonc",
];

/// files.mjs `viewerKind(name)`.
pub fn viewer_kind(name: &str) -> &'static str {
    let t = ext_of(name);
    let t = t.as_str();
    if t.is_empty() {
        return "unknown";
    }
    if IMAGE.contains(&t) {
        return "image";
    }
    if VIDEO.contains(&t) {
        return "video";
    }
    if AUDIO.contains(&t) {
        return "audio";
    }
    match t {
        "pdf" => "pdf",
        _ if TABLE.contains(&t) => "table",
        "json" => "json",
        "md" | "markdown" | "mdx" => "markdown",
        "html" | "htm" => "html",
        "docx" => "docx",
        "pptx" | "ppt" | "odp" | "key" => "pptx",
        _ if TEXTUAL.contains(&t) => "text",
        _ => "unknown",
    }
}

fn kind_by_ext(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "docx" | "doc" | "odt" | "rtf" | "md" | "markdown" | "txt" | "pages" => "document",
        "pptx" | "ppt" | "key" | "odp" => "presentation",
        "xlsx" | "xlsm" | "xls" | "csv" | "tsv" | "numbers" | "ods" => "spreadsheet",
        "pdf" => "pdf",
        _ => return None,
    })
}

fn s(v: Option<&Value>) -> Option<String> {
    v.filter(|x| js::truthy(x)).map(js::string_of)
}

/// The name a file record goes by: `f.name || f.path || ""`.
fn name_or_path(f: &Value) -> String {
    s(f.get("name")).or_else(|| s(f.get("path"))).unwrap_or_default()
}

/// `fileKind(f)`: the Library kind of a reply file.
pub fn file_kind(f: &Value) -> &'static str {
    let ext = s(f.get("ext")).unwrap_or_else(|| ext_of(&name_or_path(f)));
    if let Some(k) = kind_by_ext(&ext) {
        return k;
    }
    let vk = s(f.get("kind")).unwrap_or_else(|| viewer_kind(&name_or_path(f)).to_owned());
    match vk.as_str() {
        "audio" | "video" => "media",
        "image" => "image",
        _ => "file",
    }
}

/// `path.join(a, b)`, normalized as Node does (`.` and `..` resolved).
pub fn join_norm(base: &Path, rel: &str) -> PathBuf {
    let joined = base.join(rel.trim_start_matches('/'));
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// config.mjs `outFile(rel)`: a path under generated/files, else `None`.
pub fn out_file(ctx: &HubCtx, rel: &str) -> Option<PathBuf> {
    let base = gen_dir(ctx).join("files");
    let p = join_norm(&base, rel);
    p.display().to_string().starts_with(&format!("{}/", base.display())).then_some(p)
}

/// routes/files.mjs `versionsOfFile(rel).at(-1)?.v || 0`.
fn last_file_version(ctx: &HubCtx, rel: &str) -> i64 {
    let Some(dir) = out_file(ctx, &format!(".versions/{rel}")) else { return 0 };
    let Ok(list) = std::fs::read_dir(dir) else { return 0 };
    list.filter_map(Result::ok)
        .filter_map(|e| {
            let f = e.file_name().to_string_lossy().into_owned();
            let d: String = f.strip_prefix('v')?.chars().take_while(char::is_ascii_digit).collect();
            (!d.is_empty() && f[1 + d.len()..].starts_with('.')).then(|| d.parse::<i64>().ok()).flatten()
        })
        .max()
        .unwrap_or(0)
}

/// `pathToFileURL(p).href`.
pub fn file_url(p: &Path) -> String {
    let mut out = String::from("file://");
    for b in p.display().to_string().bytes() {
        if b <= 0x20 || b >= 0x7f || b"\"#%<>?`{}\\".contains(&b) {
            out.push_str(&format!("%{b:02X}"));
        } else {
            out.push(b as char);
        }
    }
    out
}

fn storage_mac(p: Option<&Path>) -> Value {
    json!({"where": "mac", "uri": p.map(file_url).unwrap_or_default()})
}

// ————— the Library's document facts —————

type DocCache = Mutex<HashMap<PathBuf, (f64, Map<String, Value>)>>;

fn doc_cache() -> &'static DocCache {
    static C: OnceLock<DocCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// files.mjs `docMeta(path)`: a PDF's pages, a deck's slides, a workbook's
/// sheet names, a Word file's page count when it recorded one; read once
/// per file version.
pub fn doc_meta(path: &Path) -> Map<String, Value> {
    let Some(mtime) = mtime_ms(path) else { return Map::new() };
    if let Some((at, m)) = doc_cache().lock().unwrap_or_else(|e| e.into_inner()).get(path) {
        if *at == mtime {
            return m.clone();
        }
    }
    let ext = ext_of(&path.display().to_string());
    let mut meta = Map::new();
    if ext == "pdf" {
        if let Ok(bytes) = std::fs::read(path) {
            let n = count_pages(&bytes);
            if n > 0 {
                meta.insert("pages".into(), json!(n));
            }
        }
    } else if ["docx", "pptx", "xlsx", "xlsm"].contains(&ext.as_str()) {
        if let Some(z) = std::fs::read(path).ok().and_then(|b| zip::Zip::read(b)) {
            if ext == "pptx" {
                let n = z.names().filter(|f| f.strip_prefix("ppt/slides/slide").and_then(|r| r.strip_suffix(".xml")).is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))).count();
                if n > 0 {
                    meta.insert("slides".into(), json!(n));
                }
            } else if ext == "docx" {
                let n = z.text("docProps/app.xml").and_then(|x| {
                    let at = x.find("<Pages>")? + 7;
                    let end = x[at..].find("</Pages>")?;
                    let d = &x[at..at + end];
                    (!d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())).then(|| d.parse::<f64>().ok()).flatten()
                });
                if let Some(n) = n.filter(|n| *n != 0.0) {
                    meta.insert("pages".into(), num(n));
                }
            } else {
                let wb = z.text("xl/workbook.xml").unwrap_or_default();
                let names = sheet_names(&wb);
                if !names.is_empty() {
                    meta.insert("sheets".into(), json!(names));
                }
            }
        }
    }
    doc_cache().lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_owned(), (mtime, meta.clone()));
    meta
}

/// `/\/Type\s*\/Page(?![a-zA-Z])/g` matches in the file read as latin1.
fn count_pages(b: &[u8]) -> usize {
    let mut n = 0;
    let mut i = 0;
    while let Some(at) = b[i..].windows(5).position(|w| w == b"/Type").map(|p| p + i) {
        let mut j = at + 5;
        while j < b.len() && matches!(b[j], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c | 0xa0) {
            j += 1;
        }
        if b[j..].starts_with(b"/Page") && !b.get(j + 5).is_some_and(|c| c.is_ascii_alphabetic()) {
            n += 1;
        }
        i = at + 5;
    }
    n
}

/// `<sheet … name="…">` names in a workbook, XML entities decoded.
fn sheet_names(wb: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = wb;
    while let Some(at) = rest.find("<sheet") {
        rest = &rest[at + 6..];
        if !rest.starts_with(|c: char| !(c.is_alphanumeric() || c == '_')) {
            continue;
        }
        let tag = &rest[..rest.find('>').unwrap_or(rest.len())];
        let mut search = tag;
        while let Some(p) = search.find("name=\"") {
            let before = &search[..p];
            if p == 0 || before.ends_with(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':' || c == '-')) {
                let v = &search[p + 6..];
                if let Some(end) = v.find('"') {
                    let d = v[..end].replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&");
                    out.push(d);
                }
                break;
            }
            search = &search[p + 6..];
        }
    }
    out
}

/// What docMeta reads of a zip: entry names and stored or deflated text.
mod zip {
    pub struct Zip {
        data: Vec<u8>,
        entries: Vec<(String, u16, usize, usize)>,
    }

    fn u16_at(b: &[u8], i: usize) -> Option<u16> {
        Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]))
    }
    fn u32_at(b: &[u8], i: usize) -> Option<u32> {
        Some(u32::from_le_bytes([*b.get(i)?, *b.get(i + 1)?, *b.get(i + 2)?, *b.get(i + 3)?]))
    }

    impl Zip {
        pub fn read(data: Vec<u8>) -> Option<Zip> {
            let eocd = (0..data.len().saturating_sub(21)).rev().find(|&i| u32_at(&data, i) == Some(0x0605_4b50))?;
            let count = u16_at(&data, eocd + 10)? as usize;
            let mut at = u32_at(&data, eocd + 16)? as usize;
            let mut entries = Vec::new();
            for _ in 0..count {
                if u32_at(&data, at)? != 0x0201_4b50 {
                    return None;
                }
                let method = u16_at(&data, at + 10)?;
                let size = u32_at(&data, at + 20)? as usize;
                let (n, e, c) = (u16_at(&data, at + 28)? as usize, u16_at(&data, at + 30)? as usize, u16_at(&data, at + 32)? as usize);
                let local = u32_at(&data, at + 42)? as usize;
                let name = String::from_utf8_lossy(data.get(at + 46..at + 46 + n)?).into_owned();
                entries.push((name, method, local, size));
                at += 46 + n + e + c;
            }
            Some(Zip { data, entries })
        }

        pub fn names(&self) -> impl Iterator<Item = &str> {
            self.entries.iter().map(|e| e.0.as_str())
        }

        pub fn text(&self, name: &str) -> Option<String> {
            let (_, method, local, size) = self.entries.iter().find(|e| e.0 == name)?;
            let d = &self.data;
            if u32_at(d, *local)? != 0x0403_4b50 {
                return None;
            }
            let start = local + 30 + u16_at(d, local + 26)? as usize + u16_at(d, local + 28)? as usize;
            let raw = d.get(start..start + size)?;
            let bytes = match method {
                0 => raw.to_vec(),
                8 => super::inflate::inflate(raw)?,
                _ => return None,
            };
            Some(String::from_utf8_lossy(&bytes).into_owned())
        }
    }
}

/// A raw DEFLATE decoder (RFC 1951), enough for a zip entry.
mod inflate {
    struct Bits<'a> {
        d: &'a [u8],
        pos: usize,
        buf: u32,
        cnt: u32,
    }

    impl Bits<'_> {
        fn need(&mut self, n: u32) -> Option<u32> {
            while self.cnt < n {
                self.buf |= (*self.d.get(self.pos)? as u32) << self.cnt;
                self.pos += 1;
                self.cnt += 8;
            }
            let v = self.buf & ((1u32 << n) - 1);
            self.buf = if n == 32 { 0 } else { self.buf >> n };
            self.cnt -= n;
            Some(v)
        }
    }

    struct Huff {
        count: [i32; 16],
        sym: Vec<u16>,
    }

    fn build(lengths: &[u8]) -> Huff {
        let mut count = [0i32; 16];
        for &l in lengths {
            count[l as usize] += 1;
        }
        count[0] = 0;
        let mut offs = [0usize; 16];
        for i in 1..16 {
            offs[i] = offs[i - 1] + count[i - 1] as usize;
        }
        let mut sym = vec![0u16; lengths.len()];
        for (s, &l) in lengths.iter().enumerate() {
            if l != 0 {
                sym[offs[l as usize]] = s as u16;
                offs[l as usize] += 1;
            }
        }
        Huff { count, sym }
    }

    fn decode(b: &mut Bits, h: &Huff) -> Option<u16> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= b.need(1)? as i32;
            let count = h.count[len];
            if code - count < first {
                return h.sym.get((index + (code - first)) as usize).copied();
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        None
    }

    const LBASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
    const LEXT: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
    const DBASE: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
    const DEXT: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

    fn codes(b: &mut Bits, out: &mut Vec<u8>, lit: &Huff, dist: &Huff) -> Option<()> {
        loop {
            let s = decode(b, lit)? as usize;
            if s < 256 {
                out.push(s as u8);
            } else if s == 256 {
                return Some(());
            } else {
                let s = s - 257;
                let len = *LBASE.get(s)? as usize + b.need(*LEXT.get(s)? as u32)? as usize;
                let ds = decode(b, dist)? as usize;
                let d = *DBASE.get(ds)? as usize + b.need(*DEXT.get(ds)? as u32)? as usize;
                if d > out.len() {
                    return None;
                }
                let from = out.len() - d;
                for k in 0..len {
                    let c = out[from + k];
                    out.push(c);
                }
            }
        }
    }

    pub fn inflate(d: &[u8]) -> Option<Vec<u8>> {
        let mut b = Bits { d, pos: 0, buf: 0, cnt: 0 };
        let mut out = Vec::new();
        loop {
            let last = b.need(1)?;
            match b.need(2)? {
                0 => {
                    b.buf = 0;
                    b.cnt = 0;
                    let len = u16::from_le_bytes([*d.get(b.pos)?, *d.get(b.pos + 1)?]) as usize;
                    b.pos += 4;
                    out.extend_from_slice(d.get(b.pos..b.pos + len)?);
                    b.pos += len;
                }
                1 => {
                    let mut l = [0u8; 288];
                    for (i, x) in l.iter_mut().enumerate() {
                        *x = match i {
                            0..=143 => 8,
                            144..=255 => 9,
                            256..=279 => 7,
                            _ => 8,
                        };
                    }
                    codes(&mut b, &mut out, &build(&l), &build(&[5u8; 30]))?;
                }
                2 => {
                    let nlen = b.need(5)? as usize + 257;
                    let ndist = b.need(5)? as usize + 1;
                    let ncode = b.need(4)? as usize + 4;
                    const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                    let mut cl = [0u8; 19];
                    for &o in ORDER.iter().take(ncode) {
                        cl[o] = b.need(3)? as u8;
                    }
                    let lencode = build(&cl);
                    let mut lens = vec![0u8; nlen + ndist];
                    let mut i = 0;
                    while i < nlen + ndist {
                        let sym = decode(&mut b, &lencode)?;
                        if sym < 16 {
                            lens[i] = sym as u8;
                            i += 1;
                        } else {
                            let (val, rep) = match sym {
                                16 => (*lens.get(i.checked_sub(1)?)?, 3 + b.need(2)? as usize),
                                17 => (0, 3 + b.need(3)? as usize),
                                _ => (0, 11 + b.need(7)? as usize),
                            };
                            for _ in 0..rep {
                                *lens.get_mut(i)? = val;
                                i += 1;
                            }
                        }
                    }
                    codes(&mut b, &mut out, &build(&lens[..nlen]), &build(&lens[nlen..]))?;
                }
                _ => return None,
            }
            if last == 1 {
                return Some(out);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn inflates_stored_fixed_and_dynamic_blocks() {
            // stored: "hi"
            assert_eq!(super::inflate(&[0x01, 0x02, 0x00, 0xfd, 0xff, b'h', b'i']).unwrap(), b"hi");
            // fixed Huffman: zlib's raw deflate of "hello hello hello"
            let fixed = [0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc8, 0x40, 0x90, 0x00];
            assert_eq!(super::inflate(&fixed).unwrap(), b"hello hello hello");
            // dynamic Huffman: zlib level 9 of 1411 bytes of words (Python's)
            let hex = "5554496ec3300cbcf319f980ce045afd814f90502339244581e650f8f515879b8cc4b2cc65480e29ddc7eb35cee33de67a3e7e1fc7f1d65debfa93dbdfedeb789ab2f5f1fc798c255a7ff5c027b76e5e0cc340fa1eafe31306b5080580c0766d1812c2a706b41d9cb113d758887027d11077cd5cb87266f831134ce0694a5b4fd4407bf2adc38aad2601224088c5d0ac70a10ca8fed0b4ce8e819793a0d0f124a43922146cb1230a12087867b016d50aa988a368268349378f9ecea6d0fc5c18991ac71469799024c18836f033cad47a69631d8ccfe26dab79237a7a8822b2e4c584c9ac1cf15abd8d8b544faff58c66aaa82646b2909d7aca010c728af12541381b596c5728d2f9363761f26120c9294db64402d2f30442c1572e958f67054b9fb9cdd77a825535997aeb50638ef2b8d4a198bbad38ac9f91e922a7d77b94e3911a88ab8034a8a65d2946eb734c85e294ce9cce6b7bb76937809ad0edbb96a22c2b63a7014ed7eb6425e2f954b084d94e5e7cd2e55ada12a84b81ec905713b79e6687242b75f09ab07deef2f07805760d594fdd59a9f41ed9eb1f";
            let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
            let out = super::inflate(&bytes).unwrap();
            use sha1::{Digest, Sha1};
            let sum: String = Sha1::digest(&out).iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!((out.len(), sum.as_str()), (1411, "36287b948b82dec36d5ed953eb4078a1963b042c"));
        }
    }
}

// ————— records —————

/// An image's or file's own icon (an app's is generated/<name>/icon.png).
pub fn art_icon_path(ctx: &HubCtx, id: &str) -> PathBuf {
    use sha1::{Digest, Sha1};
    let hex: String = Sha1::digest(id.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    gen_dir(ctx).join("icons").join(format!("{}.png", &hex[..16]))
}

fn art_icon_url(ctx: &HubCtx, id: &str, origin: &str) -> String {
    match mtime_ms(&art_icon_path(ctx, id)) {
        Some(t) => format!("{origin}/artifact/icon?id={}&t={}", crate::hub::market::catalog::encode_component(id), js::number(js_round(t))),
        None => String::new(),
    }
}

/// `context(origin)`: the lookups one Library build shares.
pub struct Lookups {
    pub origin: String,
    pub published: HashSet<String>,
    pub apps: HashMap<String, Map<String, Value>>,
    pub edits: HashMap<String, Map<String, Value>>,
    pub hints: HashMap<String, Hint>,
}

impl Lookups {
    pub fn read(ctx: &HubCtx, origin: &str) -> Result<Self, String> {
        let key = |r: &Map<String, Value>, k: &str| r.get(k).map(js::string_of).unwrap_or_default();
        let (published, apps, edits) = ctx.db.with(|c| {
            let p = rows_of(c, "SELECT name FROM widgets", &[])?.iter().map(|r| key(r, "name")).collect();
            let a = rows_of(c, "SELECT * FROM apps", &[])?.into_iter().map(|r| (key(&r, "name"), r)).collect();
            let e = rows_of(c, "SELECT * FROM artifact_meta", &[])?.into_iter().map(|r| (key(&r, "id"), r)).collect();
            Ok((p, a, e))
        })?;
        Ok(Lookups { origin: origin.to_owned(), published, apps, edits, hints: hints(ctx) })
    }
}

fn names_of(ctx: &HubCtx, sql: &str, col: &str) -> Result<HashSet<String>, String> {
    ctx.db.with(|c| Ok(rows_of(c, sql, &[])?.iter().map(|r| r.get(col).map(js::string_of).unwrap_or_default()).collect()))
}

fn edit_field(e: Option<&Map<String, Value>>, k: &str) -> Option<String> {
    e.and_then(|e| s(e.get(k)))
}

fn origin_of(t: &Thread, i: i64, turn: &str) -> Value {
    json!({"thread": t.id, "uid": t.uid.clone().unwrap_or_default(), "i": i, "turn": turn})
}

fn turn_id(u: &Turn) -> String {
    u.id.clone().unwrap_or_default()
}

/// The record of reply file `f` on turn `i` of `t` (`p` its path).
fn file_record(ctx: &HubCtx, t: &Thread, i: usize, u: &Turn, f: &Value, p: &Path, l: &Lookups) -> Value {
    let path = f.get("path").map(js::string_of).unwrap_or_default();
    let id = format!("file:{path}");
    let e = l.edits.get(&id);
    let base = path.rsplit('/').next().unwrap_or("").to_owned();
    let filename = edit_field(e, "file").or_else(|| s(f.get("name"))).unwrap_or(base);
    let size = f.get("size").filter(|v| js::truthy(v)).cloned().unwrap_or_else(|| json!(std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)));
    let mut meta = Map::new();
    meta.insert("size".into(), size);
    meta.insert("ext".into(), json!(s(f.get("ext")).unwrap_or_else(|| ext_of(&name_or_path(f)))));
    if let (Some(w), Some(h)) = (f.get("width").filter(|v| js::truthy(v)), f.get("height").filter(|v| js::truthy(v))) {
        meta.insert("w".into(), w.clone());
        meta.insert("h".into(), h.clone());
    }
    if let Some(d) = f.get("duration").filter(|v| js::truthy(v)) {
        meta.insert("duration".into(), d.clone());
    }
    meta.extend(doc_meta(p));
    let attach = match edit_field(e, "file") {
        Some(name) => {
            let mut m = f.as_object().cloned().unwrap_or_default();
            m.insert("name".into(), json!(name));
            Value::Object(m)
        }
        None => f.clone(),
    };
    let i = i as i64;
    json!({
        "id": id,
        "name": id,
        "kind": file_kind(f),
        "title": edit_field(e, "title").unwrap_or_else(|| filename.clone()),
        "filename": filename,
        "file": path,
        "description": edit_field(e, "description").unwrap_or_default(),
        "icon": art_icon_url(ctx, &id, &l.origin),
        "meta": meta,
        "attach": attach,
        "i": i,
        "origin": origin_of(t, i, &turn_id(u)),
        "storage": storage_mac(Some(p)),
        "versioning": "rox",
        "published": false,
        "mtime": num(mtime_ms(p).unwrap_or(0.0)),
        "v": last_file_version(ctx, &path),
        "versions": [],
    })
}

fn links_of(u: &Turn) -> Vec<Value> {
    u.links.as_ref().and_then(Value::as_array).cloned().unwrap_or_default()
}

/// `driveLinks(t)`: the thread's Drive links by file id, newest first,
/// with what older ones knew filled in.
fn drive_links(t: &Thread) -> Vec<(String, Map<String, Value>)> {
    let mut out: Vec<(String, Map<String, Value>)> = Vec::new();
    for u in t.turns.iter().rev() {
        for l in links_of(u) {
            let Some(id) = s(l.get("id")).filter(|_| l.get("provider").and_then(Value::as_str) == Some("gdrive")) else { continue };
            match out.iter_mut().find(|(k, _)| *k == id) {
                None => {
                    let mut m = l.as_object().cloned().unwrap_or_default();
                    m.insert("first".into(), l.get("at").filter(|v| js::truthy(v)).cloned().unwrap_or(json!(0)));
                    out.push((id, m));
                }
                Some((_, cur)) => {
                    for k in ["name", "mime", "link"] {
                        if !cur.get(k).is_some_and(js::truthy) {
                            cur.insert(k.into(), l.get(k).filter(|v| js::truthy(v)).cloned().unwrap_or(json!("")));
                        }
                    }
                    let first = l.get("at").filter(|v| js::truthy(v)).cloned().unwrap_or_else(|| cur.get("first").cloned().unwrap_or(Value::Null));
                    cur.insert("first".into(), first);
                }
            }
        }
    }
    out
}

/// The record of Drive link `l` (merged across the thread) on turn `i`.
fn link_record(ctx: &HubCtx, t: &Thread, i: usize, u: &Turn, l: &Map<String, Value>, lk: &Lookups) -> Value {
    let id = format!("gdrive:{}", l.get("id").map(js::string_of).unwrap_or_default());
    let e = lk.edits.get(&id);
    let (kind, ty) = drive_type(l.get("mime"), l.get("name"));
    let name = s(l.get("name")).unwrap_or_default();
    let mime = s(l.get("mime")).unwrap_or_default();
    let ext = ext_of(&name);
    let mut meta = Map::new();
    meta.insert("provider".into(), json!("Google Drive"));
    meta.insert("type".into(), json!(ty));
    meta.insert("mime".into(), json!(mime));
    meta.insert("action".into(), json!(s(l.get("action")).unwrap_or_default()));
    if !ext.is_empty() && !mime.contains("google-apps") {
        meta.insert("ext".into(), json!(ext));
    }
    let link = s(l.get("link")).unwrap_or_default();
    let at = l.get("at").filter(|v| js::truthy(v)).cloned();
    let i = i as i64;
    json!({
        "id": id,
        "name": id,
        "kind": kind,
        "title": if name.is_empty() { format!("Untitled {ty}") } else { name.clone() },
        "filename": name,
        "file": "",
        "description": edit_field(e, "description").unwrap_or_default(),
        "icon": art_icon_url(ctx, &id, &lk.origin),
        "meta": meta,
        "i": i,
        "origin": origin_of(t, i, &turn_id(u)),
        "storage": {"where": "connector", "provider": "gdrive", "link": link, "uri": link},
        "versioning": "provider",
        "published": false,
        "mtime": at.clone().unwrap_or(json!(0)),
        "created_at": l.get("first").filter(|v| js::truthy(v)).cloned().or(at).unwrap_or(json!(0)),
        "v": 0,
        "versions": [],
    })
}

/// The record of attachment `a` of thread `t`.
fn attachment_record(t: &Thread, a: &Value) -> Value {
    let g = |k: &str| a.get(k).cloned().unwrap_or(Value::Null);
    let id = format!("attach:{}", js::string_of(&g("id")));
    let turn = g("turn");
    let i = t.turns.iter().position(|u| u.id.as_ref().map(|x| json!(x)) == Some(turn.clone())).map_or(-1, |i| i as i64);
    let d = a.get("details").filter(|d| js::truthy(d)).cloned().unwrap_or(json!({}));
    let dg = |k: &str| d.get(k).filter(|v| js::truthy(v)).cloned();
    let open = g("open");
    json!({
        "id": id,
        "name": id,
        "kind": "mcp-app",
        "title": g("title"),
        "filename": "",
        "file": "",
        "description": dg("subtitle").unwrap_or(json!("")),
        "icon": dg("image").unwrap_or(json!("")),
        "image": dg("image").unwrap_or(Value::Null),
        "token": g("token"),
        "open": open,
        "key": g("key"),
        "server": g("server"),
        "scope": g("scope"),
        "details": g("details"),
        "attachment": g("id"),
        "meta": {"provider": g("server"), "type": g("scope")},
        "i": i,
        "origin": {"thread": t.id, "uid": t.uid.clone().unwrap_or_default(), "i": i, "turn": if js::truthy(&turn) { turn.clone() } else { json!("") }},
        "storage": {"where": "connector", "provider": g("server"), "link": open, "uri": open},
        "versioning": "provider",
        "published": false,
        "mtime": a.get("created_at").filter(|v| js::truthy(v)).cloned().unwrap_or(json!(0)),
        "created_at": a.get("created_at").filter(|v| js::truthy(v)).cloned().unwrap_or(json!(0)),
        "v": 0,
        "versions": [],
    })
}

/// `promptBefore(t, i)`: the nearest user turn's text above turn `i`.
fn prompt_before(t: &Thread, i: usize) -> String {
    t.turns[..i].iter().rev().find(|u| u.role == "user").map(|u| u.text.clone().unwrap_or_default()).unwrap_or_default()
}

fn art_get<'a>(u: &'a Turn, k: &str) -> Option<&'a Value> {
    u.art.as_ref().and_then(|a| a.get(k)).filter(|v| js::truthy(v))
}

/// `imageFileOf(name, v)`: the generated image an image card's version
/// shows (its `asset:/generated/assets/<file>`), `None` for any other card.
pub fn image_file_of(ctx: &HubCtx, name: &str, v: Option<&str>) -> Option<String> {
    let path = match v {
        Some(v) => ver_file(&Dirs::of(&ctx.data), name, v)?,
        None => gen_dir(ctx).join(format!("{name}.tsx")),
    };
    let code = std::fs::read_to_string(path).ok()?;
    let at = code.find("asset:/generated/assets/img-")? + "asset:/generated/assets/".len();
    let file: String = code[at..].chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')).collect();
    Some(file)
}

/// The record of image card or mini app `u.name` on turn `i` of `t`.
fn card_record(ctx: &HubCtx, t: &Thread, i: usize, u: &Turn, l: &Lookups) -> Value {
    let name = u.name.clone().unwrap_or_default();
    let ii = i as i64;
    let origin = origin_of(t, ii, &turn_id(u));
    let prompt = prompt_before(t, i);
    let empty = Map::new();
    let app = l.apps.get(&name).unwrap_or(&empty);
    let versions = Value::Array(versions_of(ctx, &name).unwrap_or_default());
    let head = head_of(ctx, &name).ok().flatten().map_or(Value::Null, |v| json!(v));
    if let Some(img) = image_file_of(ctx, &name, None) {
        let e = l.edits.get(&name);
        let path = gen_dir(ctx).join("assets").join(&img);
        let named = |x: &&Turn, k: &str| x.name.as_deref() == Some(name.as_str()) && art_get(x, k).is_some();
        let art: Value = if art_get(u, "w").is_some() {
            u.art.clone().unwrap_or(json!({}))
        } else {
            t.turns.iter().find(|x| named(x, "w")).and_then(|x| x.art.clone()).or_else(|| u.art.clone().filter(js::truthy)).unwrap_or(json!({}))
        };
        let a = |k: &str| art.get(k).filter(|v| js::truthy(v)).cloned();
        let title = edit_field(e, "title")
            .or_else(|| s(app.get("title")))
            .or_else(|| t.turns.iter().find(|x| named(x, "title")).and_then(|x| art_get(x, "title")).map(js::string_of))
            .or_else(|| Some(js_slice(&prompt, 60)).filter(|p| !p.is_empty()))
            .unwrap_or_else(|| "Image".into());
        let description = match e.and_then(|e| e.get("description")).filter(|v| !v.is_null()) {
            Some(d) => d.clone(),
            None => art.get("desc").filter(|v| !v.is_null()).cloned().unwrap_or(json!("")),
        };
        let mut meta = Map::new();
        meta.insert("w".into(), a("w").unwrap_or(json!(0)));
        meta.insert("h".into(), a("h").unwrap_or(json!(0)));
        if let Some(via) = a("via") {
            meta.insert("made_with".into(), via);
        }
        return json!({
            "id": name,
            "name": name,
            "kind": "image",
            "title": title,
            "filename": edit_field(e, "file").unwrap_or_else(|| img.clone()),
            "file": img,
            "description": description,
            "icon": art_icon_url(ctx, &name, &l.origin),
            "meta": meta,
            "w": a("w").unwrap_or(json!(0)),
            "h": a("h").unwrap_or(json!(0)),
            "i": ii,
            "origin": origin,
            "storage": storage_mac(Some(&path)),
            "versioning": "rox",
            "published": false,
            "mtime": num(mtime_ms(&path).unwrap_or(0.0)),
            "v": head,
            "versions": versions,
        });
    }
    let caption = u.caption.clone().filter(|c| !c.is_empty() && !["error", "implicit", "rejected"].iter().any(|w| c.to_lowercase().contains(w))).unwrap_or_default();
    let sum = app_summary(ctx, &name, &l.origin, app, &l.hints);
    let path = gen_dir(ctx).join(format!("{name}.tsx"));
    let title = s(app.get("title"))
        .or_else(|| art_get(u, "title").map(js::string_of))
        .or_else(|| u.label.clone().filter(|x| !x.is_empty()))
        .or_else(|| Some(caption).filter(|c| !c.is_empty()))
        .or_else(|| Some(js_slice(&prompt, 60)).filter(|p| !p.is_empty()))
        .unwrap_or_else(|| name.clone());
    let at = if path.exists() { path.clone() } else { ctx.data.join("widgets").join(format!("{name}.tsx")) };
    json!({
        "id": name,
        "name": name,
        "kind": "app",
        "title": title,
        "filename": sum["file"],
        "file": sum["file"],
        "description": sum["description"],
        "icon": sum["icon"],
        "meta": {"w": sum["w"], "h": sum["h"]},
        "subtitle": js_slice(&squeeze_spaces(&prompt), 120),
        "i": ii,
        "origin": origin,
        "storage": storage_mac(Some(&at)),
        "versioning": "rox",
        "published": l.published.contains(&name),
        "mtime": num(mtime_ms(&path).unwrap_or(0.0)),
        "v": head,
        "versions": versions,
    })
}

/// `s.replace(/\s+/g, " ")` (no trim).
fn squeeze_spaces(s: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in s.chars() {
        if js::is_space(c) || matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
            if !gap {
                out.push(' ');
            }
            gap = true;
        } else {
            gap = false;
            out.push(c);
        }
    }
    out
}

fn archived(mut rec: Value, hidden: &HashSet<String>) -> Value {
    if rec.get("id").and_then(Value::as_str).is_some_and(|id| hidden.contains(id)) {
        rec["archived"] = json!(true);
    }
    rec
}

fn files_of(u: &Turn) -> Vec<Value> {
    u.files.as_ref().and_then(Value::as_array).cloned().unwrap_or_default()
}

/// `GET /thread/apps` (`libraryOf(t, origin)`): thread `thread`'s
/// artifacts, newest first; archived ones flagged, deleted and missing ones
/// left out.
pub fn library_of(ctx: &HubCtx, thread: &str, origin: &str) -> Result<Vec<Value>, String> {
    let Some(t) = ctx.store.read(|s| s.list.iter().find(|t| t.id == thread).cloned()) else { return Ok(Vec::new()) };
    let l = Lookups::read(ctx, origin)?;
    let tomb = names_of(ctx, "SELECT name FROM tombstones", "name")?;
    let hidden = names_of(ctx, "SELECT id FROM library_hidden", "id")?;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    let links = drive_links(&t);
    let atts: Vec<Value> = ctx.db.with(|c| Ok(attachments::attachments_of(c, &t.id)))?.iter().map(|a| attachment_record(&t, a)).collect();
    let gen = gen_dir(ctx);
    let widgets = ctx.data.join("widgets");
    for i in (0..t.turns.len()).rev() {
        let u = &t.turns[i];
        for rec in atts.iter().filter(|r| r["i"] == json!(i)) {
            out.push(archived(rec.clone(), &hidden));
        }
        if u.role != "ai" || u.building {
            continue;
        }
        for lk in links_of(u) {
            let lid = lk.get("id").map(js::string_of).unwrap_or_else(|| "undefined".into());
            let id = format!("gdrive:{lid}");
            let Some((_, merged)) = links.iter().find(|(k, _)| *k == lid) else { continue };
            if seen.contains(&id) {
                continue;
            }
            seen.insert(id);
            out.push(archived(link_record(ctx, &t, i, u, merged, &l), &hidden));
        }
        for f in files_of(u) {
            let Some(path) = s(f.get("path")) else { continue };
            let id = format!("file:{path}");
            if seen.contains(&id) {
                continue;
            }
            seen.insert(id);
            let Some(p) = out_file(ctx, &path).filter(|p| p.exists()) else { continue };
            out.push(archived(file_record(ctx, &t, i, u, &f, &p, &l), &hidden));
        }
        let Some(name) = u.name.clone().filter(|n| !n.is_empty()) else { continue };
        if seen.contains(&name) || tomb.contains(&name) {
            continue;
        }
        seen.insert(name.clone());
        if !gen.join(format!("{name}.tsx")).exists() && !widgets.join(format!("{name}.tsx")).exists() {
            continue;
        }
        out.push(archived(card_record(ctx, &t, i, u, &l), &hidden));
    }
    for rec in atts.iter().filter(|r| r["i"].as_i64().is_some_and(|i| i < 0)) {
        out.push(archived(rec.clone(), &hidden));
    }
    Ok(out)
}

/// Where artifact `id` was made: a thread, the turn's index, and the file
/// or merged Drive link it holds.
struct At {
    t: Thread,
    i: usize,
    f: Option<Value>,
    l: Option<Map<String, Value>>,
}

/// `locate(id, prefer)`: the newest turn holding `id`, in `prefer` when it
/// has one, else in any thread.
fn locate(ctx: &HubCtx, id: &str, prefer: Option<&str>) -> Option<At> {
    let path = id.strip_prefix("file:");
    let drive = is_drive(id).then(|| &id[7..]);
    let in_thread = |t: &Thread| -> Option<At> {
        for i in (0..t.turns.len()).rev() {
            let u = &t.turns[i];
            if u.role != "ai" || u.building {
                continue;
            }
            if let Some(d) = drive {
                if links_of(u).iter().any(|x| x.get("id").and_then(Value::as_str) == Some(d)) {
                    let l = drive_links(t).into_iter().find(|(k, _)| k == d).map(|(_, m)| m);
                    return Some(At { t: t.clone(), i, f: None, l: Some(l.unwrap_or_default()) });
                }
            } else if let Some(p) = path {
                if let Some(f) = files_of(u).into_iter().find(|x| x.get("path").and_then(Value::as_str) == Some(p)) {
                    return Some(At { t: t.clone(), i, f: Some(f), l: None });
                }
            } else if u.name.as_deref() == Some(id) {
                return Some(At { t: t.clone(), i, f: None, l: None });
            }
        }
        None
    };
    ctx.store.read(|s| {
        if let Some(p) = prefer.and_then(|p| s.list.iter().find(|t| t.id == p)) {
            if let Some(at) = in_thread(p) {
                return Some(at);
            }
        }
        s.list.iter().find_map(in_thread)
    })
}

/// `/^gdrive:./`.
fn is_drive(id: &str) -> bool {
    id.strip_prefix("gdrive:").is_some_and(|r| r.chars().next().is_some_and(|c| c != '\n' && c != '\r'))
}

/// `artifactRecords(refs, origin)`: each `(id, thread)`'s record, from its
/// own thread when that still holds it; `None` for one that is gone.
pub fn artifact_records(ctx: &HubCtx, refs: &[(String, Option<String>)], origin: &str) -> Result<Vec<Option<Value>>, String> {
    let l = Lookups::read(ctx, origin)?;
    let tomb = names_of(ctx, "SELECT name FROM tombstones", "name")?;
    let (gen, widgets) = (gen_dir(ctx), ctx.data.join("widgets"));
    Ok(refs
        .iter()
        .map(|(id, thread)| {
            let at = locate(ctx, id, thread.as_deref())?;
            let u = &at.t.turns[at.i];
            if let Some(lk) = &at.l {
                return Some(link_record(ctx, &at.t, at.i, u, lk, &l));
            }
            if let Some(f) = &at.f {
                let p = out_file(ctx, &f.get("path").map(js::string_of).unwrap_or_default()).filter(|p| p.exists())?;
                return Some(file_record(ctx, &at.t, at.i, u, f, &p, &l));
            }
            let name = u.name.clone().unwrap_or_default();
            let gone = tomb.contains(&name) || (!gen.join(format!("{name}.tsx")).exists() && !widgets.join(format!("{name}.tsx")).exists());
            (!gone).then(|| card_record(ctx, &at.t, at.i, u, &l))
        })
        .collect())
}

fn bot_name_of(ctx: &HubCtx, thread: &str) -> String {
    ctx.store.read(|s| s.bots.iter().find(|b| b.thread_id.as_deref() == Some(thread)).map(|b| b.name.clone()).unwrap_or_default())
}

/// Sets `k`, keeping its place when the record has it (JavaScript's
/// spread then assignment).
fn set(m: &mut Map<String, Value>, k: &str, v: Value) {
    m.insert(k.into(), v);
}

/// `GET /artifact/info?id=` for an image, file, Drive file or attachment
/// (`artifactInfo(id, origin)`).
pub fn artifact_info(ctx: &HubCtx, id: &str, origin: &str) -> Result<Value, String> {
    if let Some(aid) = id.strip_prefix("attach:") {
        let all = attachments::list(ctx, None, None, None)?;
        let a = all.into_iter().find(|x| x.get("id").map(js::string_of).as_deref() == Some(aid));
        let t = a.as_ref().and_then(|a| {
            let th = a.get("thread").cloned().unwrap_or(Value::Null);
            ctx.store.read(|s| s.list.iter().find(|t| Some(t.id.as_str()) == th.as_str()).cloned())
        });
        let (Some(a), Some(t)) = (a, t) else { return Err(format!("no such artifact {id}")) };
        let mut m = Map::new();
        m.insert("ok".into(), json!(true));
        m.extend(attachment_record(&t, &a).as_object().cloned().unwrap_or_default());
        set(&mut m, "versions", json!(0));
        set(&mut m, "history", json!([]));
        set(&mut m, "thread_id", json!(t.id));
        set(&mut m, "thread_title", json!(t.title));
        set(&mut m, "bot_name", a.get("botName").cloned().unwrap_or(Value::Null));
        set(&mut m, "created_at", a.get("created_at").filter(|v| js::truthy(v)).cloned().unwrap_or(Value::Null));
        set(&mut m, "updated_at", Value::Null);
        set(&mut m, "prompt", json!(""));
        set(&mut m, "path", json!(""));
        set(&mut m, "bytes", json!(0));
        return Ok(Value::Object(m));
    }
    let Some(at) = locate(ctx, id, None) else { return Err(format!("no such artifact {id}")) };
    let l = Lookups::read(ctx, origin)?;
    let u = &at.t.turns[at.i];
    let bot = bot_name_of(ctx, &at.t.id);
    let mut m = Map::new();
    m.insert("ok".into(), json!(true));
    if let Some(lk) = &at.l {
        let rec = link_record(ctx, &at.t, at.i, u, lk, &l);
        m.extend(rec.as_object().cloned().unwrap_or_default());
        set(&mut m, "versions", json!(0));
        set(&mut m, "history", json!([]));
        set(&mut m, "thread_id", json!(at.t.id));
        set(&mut m, "thread_title", json!(at.t.title));
        set(&mut m, "bot_name", json!(bot));
        set(&mut m, "created_at", rec.get("created_at").filter(|v| js::truthy(v)).cloned().unwrap_or(Value::Null));
        set(&mut m, "updated_at", rec.get("mtime").filter(|v| js::truthy(v)).cloned().unwrap_or(Value::Null));
        set(&mut m, "prompt", json!(""));
        set(&mut m, "path", json!(""));
        set(&mut m, "bytes", json!(0));
        return Ok(Value::Object(m));
    }
    let rec = match &at.f {
        Some(f) => {
            let fp = f.get("path").map(js::string_of).unwrap_or_default();
            let p = out_file(ctx, &fp).filter(|p| p.exists()).ok_or_else(|| format!("no such file {fp}"))?;
            file_record(ctx, &at.t, at.i, u, f, &p, &l)
        }
        None => card_record(ctx, &at.t, at.i, u, &l),
    };
    let uri = rec["storage"]["uri"].as_str().unwrap_or_default();
    let path = js::decode_uri_component(uri.strip_prefix("file://").unwrap_or(uri)).map_err(|_| "URI malformed".to_owned())?;
    let (bytes, born) = match std::fs::metadata(&path) {
        Ok(md) => (md.len(), born_ms(Path::new(&path)).unwrap_or(0.0)),
        Err(_) => (0, 0.0),
    };
    let rid = rec["id"].as_str().unwrap_or_default().to_owned();
    let first = if at.f.is_some() {
        Value::Null
    } else {
        ctx.db.with(|c| c.query_row("SELECT MIN(created_at) AS c FROM versions WHERE name=?", [&rid], |r| r.get::<_, SqlValue>(0)).map(crate::hub::store::write::to_json))?
    };
    let updated = ctx.db.with(|c| row_of(c, "SELECT * FROM artifact_meta WHERE id=?", &[&rid]))?.and_then(|r| r.get("updated_at").cloned()).unwrap_or(Value::Null);
    let versions = rec["versions"].clone();
    m.extend(rec.as_object().cloned().unwrap_or_default());
    let fnum = crate::hub::store::json::js_number(Some(&first));
    let created = js_round(if fnum != 0.0 && !fnum.is_nan() { fnum } else { born });
    let updated = crate::hub::store::json::js_number(Some(&updated));
    set(&mut m, "versions", json!(versions.as_array().map_or(0, Vec::len)));
    set(&mut m, "history", versions);
    set(&mut m, "thread_id", json!(at.t.id));
    set(&mut m, "thread_title", json!(at.t.title));
    set(&mut m, "bot_name", json!(bot));
    set(&mut m, "created_at", if created != 0.0 && !created.is_nan() { num(created) } else { Value::Null });
    set(&mut m, "updated_at", if updated != 0.0 && !updated.is_nan() { num(updated) } else { Value::Null });
    set(&mut m, "prompt", json!(if at.f.is_some() { String::new() } else { prompt_before(&at.t, at.i) }));
    set(&mut m, "path", json!(path));
    set(&mut m, "bytes", json!(bytes));
    Ok(Value::Object(m))
}

/// `isApp(id)`: an R-name that is no image card.
pub fn is_app(ctx: &HubCtx, id: &str) -> bool {
    is_r_name(id) && image_file_of(ctx, id, None).is_none()
}

/// routes/artifacts.mjs `info(id)`: an app's /app/info, else its record.
pub fn info(ctx: &HubCtx, id: &str, origin: &str) -> Result<Value, String> {
    if is_app(ctx, id) {
        app_info(ctx, id, origin)
    } else {
        artifact_info(ctx, id, origin)
    }
}

/// `POST /artifact/meta` (`updateArtifactMeta(id, body)`): an app's edit
/// goes to `apps`, an image's or file's to `artifact_meta`; the extension
/// is kept, files on disk never renamed; icon "" removes the icon.
pub async fn update_artifact_meta(ctx: &HubCtx, id: &str, body: &Value) -> Result<(), String> {
    let get = |k: &str| body.as_object().and_then(|o| o.get(k));
    let filename = get("filename").or_else(|| get("file"));
    if is_app(ctx, id) {
        let mut m = Map::new();
        for (k, v) in [("title", get("title")), ("file", filename), ("description", get("description")), ("icon", get("icon"))] {
            if let Some(v) = v {
                m.insert(k.into(), v.clone());
            }
        }
        return update_app_meta(ctx, id, &Value::Object(m)).await;
    }
    let Some(at) = locate(ctx, id, None) else { return Err(format!("no such artifact {id}")) };
    let ext = match (&at.f, &at.l) {
        (Some(f), _) => s(f.get("ext")).unwrap_or_else(|| ext_of(&name_or_path(f))),
        (None, Some(_)) => String::new(),
        _ => ext_of(&image_file_of(ctx, id, None).unwrap_or_default()),
    };
    let mut cols: Vec<(&str, SqlValue)> = vec![("updated_at", SqlValue::Integer(now_ms() as i64))];
    if let Some(t) = get("title") {
        let t = js_slice(trim_js(&str_or_empty(Some(t))), 80);
        cols.push(("title", if t.is_empty() { SqlValue::Null } else { SqlValue::Text(t) }));
    }
    if let Some(f) = filename {
        let mut stem: String = trim_js(&str_or_empty(Some(f))).chars().map(|c| if matches!(c, '/' | '\\' | ':') { '-' } else { c }).collect();
        if !ext.is_empty() && stem.to_lowercase().ends_with(&format!(".{ext}")) {
            stem = js_slice(&stem, crate::hub::store::json::utf16_len(&stem) - ext.encode_utf16().count() - 1);
        }
        let stem = js_slice(trim_js(&stem), 120);
        cols.push(("file", if stem.is_empty() { SqlValue::Null } else { SqlValue::Text(if ext.is_empty() { stem } else { format!("{stem}.{ext}") }) }));
    }
    if let Some(d) = get("description") {
        cols.push(("description", SqlValue::Text(js_slice(trim_js(&str_or_empty(Some(d))), 280))));
    }
    if let Some(icon) = get("icon") {
        let out = art_icon_path(ctx, id);
        if trim_js(&str_or_empty(Some(icon))).is_empty() {
            let _ = std::fs::remove_file(&out);
            cols.push(("icon", SqlValue::Null));
        } else {
            let _ = std::fs::create_dir_all(gen_dir(ctx).join("icons"));
            cols.push(("icon", SqlValue::Text(adopt_icon(ctx, &js::string_of(icon), &out).await?)));
        }
    }
    ctx.db.with(|c| {
        c.execute("INSERT INTO artifact_meta(id) VALUES(?) ON CONFLICT(id) DO NOTHING", [id])?;
        let set = cols.iter().map(|(k, _)| format!("{k}=?")).collect::<Vec<_>>().join(",");
        let args: Vec<SqlValue> = cols.iter().map(|(_, v)| v.clone()).chain(std::iter::once(SqlValue::Text(id.to_owned()))).collect();
        c.execute(&format!("UPDATE artifact_meta SET {set} WHERE id=?"), params_from_iter(args))?;
        Ok(())
    })
}

/// `POST /artifact/hide` (`hideArtifact(id, hidden)`).
pub fn hide_artifact(ctx: &HubCtx, id: &str, hidden: bool) -> Result<(), String> {
    if !is_r_name(id) && !id.starts_with("file:") && !is_drive(id) && !id.starts_with("attach:") {
        return Err(format!("bad artifact id {id}"));
    }
    ctx.db.with(|c| {
        if hidden {
            c.execute("INSERT OR REPLACE INTO library_hidden(id,at) VALUES(?,?)", rusqlite::params![id, now_ms() as i64])?;
        } else {
            c.execute("DELETE FROM library_hidden WHERE id=?", [id])?;
        }
        Ok(())
    })
}

/// `forgetArtifact(id)`: its group memberships (any version's ref), archive
/// flag and edited details.
pub fn forget_artifact(ctx: &HubCtx, id: &str) -> Result<(), String> {
    ctx.db.with(|c| {
        c.execute("DELETE FROM group_items WHERE kind='artifact' AND (ref=? OR ref LIKE ?)", [id, &format!("{id}@v%")])?;
        c.execute("DELETE FROM library_hidden WHERE id=?", [id])?;
        c.execute("DELETE FROM artifact_meta WHERE id=?", [id])?;
        Ok(())
    })
}

/// `POST /artifact/delete` (`deleteArtifact(id)`): a reply file leaves
/// every chat, the disk, its groups and the Library.
pub fn delete_artifact(ctx: &HubCtx, id: &str) -> Result<(), String> {
    let Some(path) = id.strip_prefix("file:") else { return Err(format!("{id} isn't a file — use /app/delete")) };
    ctx.store.write(|s| {
        for t in s.list.iter_mut() {
            for u in t.turns.iter_mut() {
                if let Some(Value::Array(fs)) = &mut u.files {
                    if !fs.is_empty() {
                        fs.retain(|f| f.get("path").and_then(Value::as_str) != Some(path));
                    }
                }
            }
            let has = |v: &Option<Value>| v.as_ref().and_then(Value::as_array).is_some_and(|a| !a.is_empty());
            t.turns
                .retain(|u| u.role != "ai" || u.building || u.name.as_deref().is_some_and(|n| !n.is_empty()) || !trim_js(u.text.as_deref().unwrap_or("")).is_empty() || has(&u.files) || has(&u.links));
        }
    })?;
    ctx.store.write_index();
    forget_artifact(ctx, id)?;
    if let Some(p) = out_file(ctx, path) {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

/// mcp.mjs `callTool(row, tool, input)`: the content folded to one string
/// and whether it is an error; a failed request is an `Err`.
pub async fn call_tool(ctx: &HubCtx, id: &str, tool: &str, input: &Value) -> Result<(String, bool), String> {
    let Some(mcp) = ctx.mcp.clone() else { return Ok((format!("mcp server {id} is not connected (failed to start)"), true)) };
    let r = mcp.ensure(id).await;
    if r.get("ok") != Some(&Value::Bool(true)) {
        let err = r.get("err").filter(|e| js::truthy(e)).map(js::string_of).unwrap_or_else(|| "failed to start".into());
        return Ok((format!("mcp server {id} is not connected ({err})"), true));
    }
    if mcp.status(id).get("status").and_then(Value::as_str) == Some("needs-auth") {
        return Ok((format!("{id} needs sign-in — ask the user to open Connect Apps, find this app and press Authenticate"), true));
    }
    let result = mcp.request(id, "tools/call", json!({"name": tool, "arguments": input}), std::time::Duration::from_secs(60)).await.map_err(|e| e.message)?;
    let parts: Vec<String> = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|c| match c.get("type").and_then(Value::as_str) {
            Some("text") => c.get("text").filter(|t| !t.is_null()).map(js::string_of).unwrap_or_default(),
            Some("image") => {
                let mime = c.get("mimeType").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_else(|| "image/png".into());
                let len = c.get("data").and_then(Value::as_str).map_or(0, |d| d.encode_utf16().count());
                format!("[image: {mime}, {} bytes]", js::number(js_round(len as f64 * 0.75)))
            }
            _ => js::stringify(c),
        })
        .collect();
    let text = parts.join("\n");
    Ok((if text.is_empty() { "(empty)".into() } else { text }, result.get("isError").is_some_and(js::truthy)))
}

/// `POST /artifact/rename-remote` (`renameRemote(id, name, origin)`):
/// renames a Drive file where it lives and records the rename on the
/// newest turn holding it; answers the artifact's info.
pub async fn rename_remote(ctx: &HubCtx, id: &str, name: &str, origin: &str) -> Result<Value, String> {
    let name = js_slice(trim_js(name), 200);
    if !is_drive(id) {
        return Err(format!("{id} is not stored in a connected app"));
    }
    if name.is_empty() {
        return Err("a file needs a name".into());
    }
    let Some(at) = locate(ctx, id, None) else { return Err(format!("no such artifact {id}")) };
    let has_row = ctx.db.with(|c| c.query_row("SELECT 1 FROM mcp_servers WHERE id=?", [DRIVE_SERVER], |_| Ok(())).optional())?.is_some();
    if !has_row {
        return Err("Google Drive isn't connected".into());
    }
    let lid = at.l.as_ref().and_then(|l| l.get("id")).cloned().unwrap_or(Value::Null);
    let mut input = Map::new();
    input.insert("file_id".into(), lid.clone());
    input.insert("name".into(), json!(name));
    let u = &at.t.turns[at.i];
    let held = links_of(u).into_iter().find(|x| x.get("id") == Some(&lid));
    if let Some(a) = held.as_ref().and_then(|x| x.get("account")).filter(|a| js::truthy(a)) {
        input.insert("user_google_email".into(), a.clone());
    }
    let input = Value::Object(input);
    let (content, is_error) = call_tool(ctx, DRIVE_SERVER, "update_drive_file", &input).await?;
    let error_text = content.trim_start_matches(|c: char| js::is_space(c) || matches!(c, '\n' | '\r'));
    let starts_error = error_text.get(..5).is_some_and(|w| w.eq_ignore_ascii_case("error")) && !error_text[5..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_');
    if is_error || starts_error {
        return Err(js_slice(&content, 300));
    }
    let link = drive_write("update_drive_file", &input, &content).and_then(|l| l.get("link").filter(|v| js::truthy(v)).cloned());
    let uid = u.id.clone();
    let now = now_ms();
    ctx.store.write(|s| {
        let Some(t) = s.list.iter_mut().find(|t| t.id == at.t.id) else { return };
        let Some(turn) = t.turns.iter_mut().find(|x| x.id == uid) else { return };
        let Some(Value::Array(ls)) = &mut turn.links else { return };
        if let Some(Value::Object(cur)) = ls.iter_mut().find(|x| x.get("id") == Some(&lid)) {
            cur.insert("name".into(), json!(name));
            cur.insert("action".into(), json!("renamed"));
            cur.insert("at".into(), num(now as f64));
            if let Some(link) = &link {
                cur.insert("link".into(), link.clone());
            }
        }
    })?;
    ctx.store.write_index();
    artifact_info(ctx, id, origin)
}

// ————— modules —————

/// tools.mjs `imageModule(file, prompt, w, h)`: an image card's module.
pub fn image_module(file: &str, _prompt: &str, w: Option<f64>, h: Option<f64>) -> String {
    let ar = match (w.filter(|w| *w != 0.0 && !w.is_nan()), h.filter(|h| *h != 0.0 && !h.is_nan())) {
        (Some(w), Some(h)) => format!(", aspectRatio: {:.4}", w / h),
        _ => String::new(),
    };
    format!(
        r#"import {{ signal, Text, View }} from "nsh";
import {{ Image }} from "nsh/image";

export default function R() {{
  const failed = signal(false);
  return (
    <View class="gap-2">
      <If when={{!failed()}}>
        <Image src="asset:/generated/assets/{file}" resizeMode="cover" onError={{() => failed.set(true)}} style={{{{ width: "100%"{ar}, borderRadius: 12 }}}} />
      </If>
      <If when={{failed()}}>
        <Text class="text-sm text-zinc-500">image failed to load</Text>
      </If>
    </View>
  );
}}
"#
    )
}

/// mcp.mjs `parseAppKey(key)`: `mcp:<server>/<app>[|<props JSON>]`.
pub fn parse_app_key(key: &str) -> Option<(String, String, Map<String, Value>)> {
    let rest = key.strip_prefix("mcp:")?;
    let (head, props) = match rest.split_once('|') {
        Some((h, p)) => (h, Some(p)),
        None => (rest, None),
    };
    let (server, app) = head.split_once('/')?;
    let ok_server = !server.is_empty() && server.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let ok_app = !app.is_empty() && app.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if !ok_server || !ok_app {
        return None;
    }
    let props = match props.filter(|p| !p.is_empty()).map(serde_json::from_str::<Value>) {
        Some(Ok(Value::Object(m))) => m,
        Some(Ok(Value::Array(a))) => a.into_iter().enumerate().map(|(i, v)| (i.to_string(), v)).collect(),
        _ => Map::new(),
    };
    Some((server.to_owned(), app.to_owned(), props))
}

/// `GET /module?name=mcp:<server>/<app>[|<props>]` (`mcpAppModule(raw)`):
/// the shipped module with its props ahead of it as `globalThis.roxProps`.
/// A failed read is an `Err` (Node's route throws).
pub async fn mcp_app_module(ctx: &HubCtx, raw: &str) -> Result<Value, String> {
    let k = parse_app_key(raw);
    let row = match &k {
        Some((server, _, _)) => ctx.db.with(|c| c.query_row("SELECT id FROM mcp_servers WHERE id=? AND enabled=1", [server], |r| r.get::<_, String>(0)).optional())?,
        None => None,
    };
    let (Some((server, app, props)), Some(row)) = (k, row) else { return Ok(json!({"source": "", "error": "no such server"})) };
    let resources = match &ctx.mcp {
        Some(mcp) => mcp.resources(&row).await,
        None => Vec::new(),
    };
    let found = resources
        .into_iter()
        .find(|r| r.get("mimeType").and_then(Value::as_str) == Some("text/x-nsh-tsx") && r.get("uri").map(js::string_of).unwrap_or_default().rsplit('/').next() == Some(app.as_str()));
    let Some(res) = found else { return Ok(json!({"source": "", "error": format!("{server} ships no app {app}")})) };
    let text = match &ctx.mcp {
        Some(mcp) => {
            let uri = res.get("uri").cloned().unwrap_or(Value::Null);
            let read = mcp.request(&row, "resources/read", json!({"uri": uri}), crate::helper::mcp::DEFAULT_TIMEOUT).await.map_err(|e| e.message)?;
            read.get("contents").and_then(Value::as_array).and_then(|c| c.iter().find_map(|x| x.get("text").and_then(Value::as_str).map(str::to_owned))).unwrap_or_default()
        }
        None => String::new(),
    };
    let mut p = props;
    p.insert("server".into(), json!(row));
    let source = if text.is_empty() { String::new() } else { format!("globalThis.roxProps = {};\n{text}", js::stringify(&Value::Object(p))) };
    let title = ["title", "name"].iter().find_map(|k| res.get(*k).filter(|v| js::truthy(v)).cloned()).unwrap_or(json!(app));
    Ok(json!({"source": source, "title": title}))
}

/// The seams track A declared, filled with the Library.
pub fn seams(ctx: &mut HubCtx) {
    use crate::hub::library::groups::ArtifactRecords;
    use crate::hub::routes::chat::LibraryOf;
    ctx.ext.insert(LibraryOf(Arc::new(|ctx: Arc<HubCtx>, thread: String, origin: String| -> BoxFuture<'static, Value> {
        Box::pin(async move { Value::Array(library_of(&ctx, &thread, &origin).unwrap_or_default()) })
    })));
    ctx.ext.insert(ArtifactRecords(Arc::new(|ctx: Arc<HubCtx>, refs: Vec<(String, Option<String>)>, origin: String| -> BoxFuture<'static, Vec<Option<Value>>> {
        Box::pin(async move {
            let n = refs.len();
            artifact_records(&ctx, &refs, &origin).unwrap_or_else(|_| vec![None; n])
        })
    })));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_file_helpers_read_as_node() {
        assert_eq!(ext_of("a/b/Report.MD"), "md");
        assert_eq!(ext_of(".env"), "");
        assert_eq!(ext_of("x."), "");
        assert_eq!(viewer_kind("a.png"), "image");
        assert_eq!(viewer_kind("a.md"), "markdown");
        assert_eq!(file_kind(&json!({"path": "m/report.md", "name": "report.md", "ext": "md"})), "document");
        assert_eq!(file_kind(&json!({"name": "a.mov"})), "media");
        assert_eq!(join_norm(Path::new("/d/files"), "m/../x.md"), PathBuf::from("/d/files/x.md"));
        assert_eq!(join_norm(Path::new("/d/files"), "../x.md"), PathBuf::from("/d/x.md"));
        assert_eq!(file_url(Path::new("/a b/c#d%.png")), "file:///a%20b/c%23d%25.png");
        assert_eq!(count_pages(b"<< /Type /Page >> << /Type/Pages >> /Type /Page/"), 2);
        assert_eq!(sheet_names(r#"<sheets><sheet name="A &amp; B" sheetId="1"/><sheet sheetId="2" name="C"/></sheets>"#), ["A & B", "C"]);
        assert_eq!(parse_app_key("mcp:apps/hello|{\"a\":1}").map(|k| (k.0, k.1, Value::Object(k.2))), Some(("apps".into(), "hello".into(), json!({"a": 1}))));
        assert!(parse_app_key("mcp:Apps/hello").is_none());
        assert!(is_drive("gdrive:x") && !is_drive("gdrive:"));
        assert!(image_module("img-a.png", "", Some(3.0), Some(2.0)).contains(r#"style={{ width: "100%", aspectRatio: 1.5000, borderRadius: 12 }}"#));
    }
}
