//! A reply's files as the model side of files.mjs makes them: `fileMeta`,
//! `fileRecord` (sizes through `sips`, posters through Quick Look, audio
//! through `afinfo`/`afconvert`, a feed derivative), `isAnimated`,
//! `feedVariant`, and the deterministic mock media (`pngEncode`,
//! `mockImages`, `mockFiles`, `mockDocx`). Every command goes through the
//! hub's `LocalTools`, so the tests' PATH stubs stand in.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::hub::ctx::HubCtx;
use crate::hub::traits::LocalCommand;
use crate::js;

const IMAGE: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "avif", "bmp", "tiff", "svg"];
const VIDEO: &[&str] = &["mp4", "mov", "webm", "m4v", "mkv", "avi"];
const AUDIO: &[&str] = &["mp3", "wav", "m4a", "aac", "flac", "ogg", "oga", "opus", "aiff"];
const TABLE: &[&str] = &["csv", "tsv", "xlsx", "xls"];
const GENERIC: &[&str] = &[
    "md", "markdown", "mdx", "txt", "text", "rtf", "log", "html", "htm", "xml", "yaml", "yml", "toml", "ini", "cfg", "env", "sh", "bash", "zsh", "js", "jsx", "mjs", "cjs", "ts", "tsx", "py", "rb",
    "go", "rs", "java", "kt", "swift", "c", "h", "cc", "cpp", "hpp", "cs", "php", "sql", "css", "scss", "tex",
];

/// `extOf(name)`: the last extension, lower-cased ("" for none).
pub fn ext_of(name: &str) -> String {
    let n = name.rsplit(['/', '\\']).next().unwrap_or("");
    match n.rfind('.') {
        Some(i) if i > 0 && i + 1 < n.len() => n[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// `viewerKind(name)` (Grok's `jT`).
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
        "jsonc" => "text",
        _ if GENERIC.contains(&t) => "text",
        _ => "unknown",
    }
}

/// `cardVariant(name)` (Grok's `$P`).
pub fn card_variant(name: &str) -> &'static str {
    let t = ext_of(name);
    match t.as_str() {
        "pdf" => "pdf",
        "doc" | "docx" | "odt" | "pages" => "docx",
        "xls" | "xlsx" | "xlsm" | "csv" | "tsv" | "ods" | "numbers" => "spreadsheet",
        "ppt" | "pptx" | "odp" => "ppt",
        "key" => "keynote",
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" => "zip",
        "ttf" | "otf" | "woff" | "woff2" => "font",
        "mp3" | "wav" | "m4a" | "aac" | "flac" | "ogg" | "oga" | "opus" | "aiff" => "audio",
        "json" | "jsonc" | "json5" | "ndjson" => "json",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "heif" | "avif" | "bmp" | "tiff" | "svg" => "media",
        "mp4" | "mov" | "webm" | "m4v" | "mkv" | "avi" => "video",
        t if GENERIC.contains(&t) => "generic",
        _ => "unknown",
    }
}

/// Runs a command: its stdout when it exits 0 (execFileSync), else `None`.
async fn run(ctx: &HubCtx, program: &str, args: &[&str], timeout: Option<u64>) -> Option<String> {
    let cmd = LocalCommand { program: program.into(), args: args.iter().map(|a| (*a).to_owned()).collect(), timeout: timeout.map(Duration::from_millis), ..Default::default() };
    let out = crate::helper::local::tools(ctx).run(cmd).await.ok()?;
    (out.code == Some(0)).then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `+(s.match(re)?.[1] || 0)` for `<lead>([\d.]+)`.
fn number_after(text: &str, lead: &str) -> f64 {
    let mut from = 0;
    while let Some(p) = text[from..].find(lead) {
        let at = from + p + lead.len();
        let n = text[at..].find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(text.len() - at);
        if n > 0 {
            return text[at..at + n].parse().unwrap_or(f64::NAN);
        }
        from = from + p + 1;
    }
    0.0
}

/// `+(head.match(/\s<k>="([\d.]+)/)?.[1] || 0)`.
fn attr_number(head: &str, lead: &str) -> f64 {
    for (p, _) in head.match_indices(lead) {
        if !head[..p].chars().next_back().is_some_and(js::is_space) {
            continue;
        }
        let at = p + lead.len();
        let n = head[at..].find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(head.len() - at);
        if n > 0 {
            return head[at..at + n].parse().unwrap_or(f64::NAN);
        }
    }
    0.0
}

/// A JS number as JSON (NaN is `null`).
fn num(f: f64) -> Value {
    if !f.is_finite() {
        Value::Null
    } else if f.fract() == 0.0 && f.abs() < 9e15 {
        json!(f as i64)
    } else {
        json!(f)
    }
}

/// `(+x || +y)`: the first, else the second when the first is 0 or NaN.
fn or(a: f64, b: f64) -> f64 {
    if a == 0.0 || a.is_nan() {
        b
    } else {
        a
    }
}

/// An SVG's `viewBox="… w h"`, as files.mjs's lazy regex reads it.
fn view_box(head: &str) -> Option<(f64, f64)> {
    let at = head.find("viewBox=\"")? + "viewBox=\"".len();
    let body = &head[at..at + head[at..].find('"')?];
    let digit = |c: char| c.is_ascii_digit() || c == '.';
    for k in 1..body.len() {
        if !body.is_char_boundary(k) || !body[..k].chars().all(|c| digit(c) || c == '-' || js::is_space(c)) {
            if !body.is_char_boundary(k) {
                continue;
            }
            return None;
        }
        let rest = &body[k..];
        let a = rest.find(|c: char| !digit(c)).unwrap_or(rest.len());
        if a == 0 {
            continue;
        }
        let after = &rest[a..];
        let ws = after.len() - after.trim_start_matches(js::is_space).len();
        let b = &after[ws..];
        if ws > 0 && !b.is_empty() && b.chars().all(digit) {
            return Some((rest[..a].parse().unwrap_or(f64::NAN), b.parse().unwrap_or(f64::NAN)));
        }
    }
    None
}

/// `imageSize(path)`: an SVG's own size, else `sips`'s; `{}` when unknown.
pub async fn image_size(ctx: &HubCtx, path: &Path) -> Map<String, Value> {
    let mut m = Map::new();
    if path.to_string_lossy().to_lowercase().ends_with(".svg") {
        let Ok(text) = std::fs::read_to_string(path) else { return m };
        let text = js::slice_utf16(&text, 2000);
        let lower = text.to_ascii_lowercase();
        let Some(start) = lower.find("<svg") else {
            m.insert("width".into(), json!(0));
            m.insert("height".into(), json!(0));
            return m;
        };
        let head = text[start..].find('>').map(|e| &text[start..start + e + 1]).unwrap_or("");
        let vb = view_box(head);
        m.insert("width".into(), num(or(attr_number(head, "width=\""), vb.map_or(0.0, |v| v.0))));
        m.insert("height".into(), num(or(attr_number(head, "height=\""), vb.map_or(0.0, |v| v.1))));
        return m;
    }
    let p = path.to_string_lossy();
    if let Some(o) = run(ctx, "sips", &["-g", "pixelWidth", "-g", "pixelHeight", &p], None).await {
        m.insert("width".into(), num(number_after(&o, "pixelWidth: ")));
        m.insert("height".into(), num(number_after(&o, "pixelHeight: ")));
    }
    m
}

/// `videoInfo(path)`: a Quick Look poster and the duration.
async fn video_info(ctx: &HubCtx, path: &Path) -> Map<String, Value> {
    let mut out = Map::new();
    let p = path.to_string_lossy().into_owned();
    let dir = &p[..p.rfind('/').unwrap_or(0)];
    if run(ctx, "qlmanage", &["-t", "-s", "640", "-o", dir, &p], Some(20000)).await.is_some() {
        let poster = PathBuf::from(format!("{p}.png"));
        if poster.exists() {
            out.insert("poster".into(), json!(format!("{}.png", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())));
            out.extend(image_size(ctx, &poster).await);
        }
    }
    if let Some(info) = run(ctx, "afinfo", &[&p], None).await {
        out.insert("duration".into(), num(number_after(&info, "estimated duration: ")));
    }
    out
}

/// `audioInfo(path)`: the duration and a 48-peak waveform.
async fn audio_info(ctx: &HubCtx, path: &Path) -> Map<String, Value> {
    let p = path.to_string_lossy().into_owned();
    let got = async {
        let info = run(ctx, "afinfo", &[&p], None).await?;
        let duration = number_after(&info, "estimated duration: ");
        let tmp = format!("/tmp/peaks-{}-{}.wav", std::process::id(), crate::hub::store::now_ms());
        run(ctx, "afconvert", &["-f", "WAVE", "-d", "LEI16@8000", "-c", "1", &p, &tmp], None).await?;
        let buf = std::fs::read(&tmp).ok()?;
        let data = buf.windows(4).position(|w| w == b"data").map(|i| i as i64).unwrap_or(-1);
        let off = data + 8;
        // an Int16Array at an odd offset throws (Node's RangeError)
        if off % 2 != 0 || off < 0 || off as usize > buf.len() {
            return None;
        }
        let pcm: Vec<i16> = buf[off as usize..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        let n = 48;
        let step = (pcm.len() / n).max(1);
        let raw: Vec<f64> = (0..n)
            .map(|i| {
                let sum: f64 = (i * step..pcm.len().min((i + 1) * step)).map(|j| (pcm[j] as f64) * (pcm[j] as f64)).sum();
                (sum / step as f64).sqrt()
            })
            .collect();
        let max = raw.iter().cloned().fold(1.0_f64, f64::max);
        Some((duration, raw.iter().map(|v| num(((v / max) * 100.0 + 0.5).floor() / 100.0)).collect::<Vec<_>>()))
    }
    .await;
    let (duration, peaks) = got.unwrap_or((0.0, Vec::new()));
    let mut m = Map::new();
    m.insert("duration".into(), num(duration));
    m.insert("peaks".into(), Value::Array(peaks));
    m
}

/// `fileMeta(path, name)`: name, ext, size, kind, variant, and the kind's
/// own facts.
pub async fn file_meta(ctx: &HubCtx, path: &Path, name: Option<&str>) -> Map<String, Value> {
    let name = name.map(str::to_owned).unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let kind = viewer_kind(&name);
    let mut m = Map::new();
    m.insert("name".into(), json!(name));
    m.insert("ext".into(), json!(ext_of(&name)));
    m.insert("size".into(), json!(size));
    m.insert("kind".into(), json!(kind));
    m.insert("variant".into(), json!(card_variant(&name)));
    if size == 0 {
        return m;
    }
    let more = match kind {
        "audio" => audio_info(ctx, path).await,
        "image" => image_size(ctx, path).await,
        "video" => video_info(ctx, path).await,
        _ => Map::new(),
    };
    m.extend(more);
    m
}

/// `isAnimated(path, ext)`: a GIF, or an animated WebP or APNG.
pub fn is_animated(path: &Path, ext: &str) -> bool {
    if ext == "gif" {
        return true;
    }
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut head = vec![0u8; 4096];
    let n = std::io::Read::read(&mut f, &mut head).unwrap_or(0);
    let b = &head[..n];
    let has = |w: &[u8]| b.windows(w.len()).any(|x| x == w);
    match ext {
        "webp" => has(b"ANMF") || has(b"ANIM"),
        "png" => has(b"acTL"),
        _ => false,
    }
}

/// `feedVariant(path, force)`: a static PNG at most 1120 px on its long
/// edge, kept when smaller than the original (or `force`).
pub async fn feed_variant(ctx: &HubCtx, path: &Path, force: bool) -> Option<PathBuf> {
    let p = path.to_string_lossy().into_owned();
    let base = match p.rfind('.') {
        Some(i) if i + 1 < p.len() => &p[..i],
        _ => &p,
    };
    let feed = format!("{base}.feed.png");
    let dir = &p[..p.rfind('/').unwrap_or(0)];
    if ext_of(&p) == "svg" {
        run(ctx, "qlmanage", &["-t", "-s", "1120", "-o", dir, &p], Some(20000)).await?;
        let png = format!("{p}.png");
        if Path::new(&png).exists() {
            std::fs::rename(&png, &feed).ok()?;
        }
    } else if run(ctx, "sips", &["-Z", "1120", &p, "--out", &feed], Some(20000)).await.is_none() {
        run(ctx, "magick", &[&format!("{p}[0]"), &feed], Some(20000)).await?;
    }
    let size = |x: &str| std::fs::metadata(x).map(|m| m.len()).ok();
    if size(&feed).is_some() && (force || size(&feed) < size(&p)) {
        return Some(PathBuf::from(feed));
    }
    if Path::new(&feed).exists() {
        let _ = std::fs::remove_file(&feed);
    }
    None
}

/// `fileRecord(p, relFile)`: the file's meta, `animated`, its feed and
/// poster as paths beside `relFile` (relative to the files dir).
pub async fn file_record(ctx: &HubCtx, p: &Path, rel: &str) -> Value {
    let m = file_meta(ctx, p, None).await;
    let base = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dir_rel = rel.strip_suffix(base.as_str()).unwrap_or("").to_owned();
    let image = m["kind"] == "image";
    let ext = js::string_of(&m["ext"]);
    let animated = image && is_animated(p, &ext);
    let feed = if image { feed_variant(ctx, p, ext == "svg" || animated).await } else { None };
    let mut out = Map::new();
    out.insert("path".into(), json!(rel));
    out.extend(m.clone());
    if animated {
        out.insert("animated".into(), json!(true));
    }
    if let Some(f) = feed {
        out.insert("feed".into(), json!(format!("{dir_rel}{}", f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())));
    }
    if let Some(poster) = m.get("poster").filter(|x| js::truthy(x)) {
        out.insert("poster".into(), json!(format!("{dir_rel}{}", js::string_of(poster))));
    }
    Value::Object(out)
}

// ————— the deterministic mock images —————

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// LSB-first bits, as deflate writes them.
struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn put(&mut self, v: u32, n: u32) {
        self.acc |= (v as u64) << self.n;
        self.n += n;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    /// A Huffman code, most significant bit first.
    fn code(&mut self, code: u32, len: u32) {
        let mut r = 0;
        for i in 0..len {
            r |= ((code >> i) & 1) << (len - 1 - i);
        }
        self.put(r, len);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

fn lit(b: &mut Bits, v: u32) {
    match v {
        0..=143 => b.code(0x30 + v, 8),
        144..=255 => b.code(0x190 + v - 144, 9),
        256..=279 => b.code(v - 256, 7),
        _ => b.code(0xc0 + v - 280, 8),
    }
}

const LEN_BASE: [u32; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA: [u32; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u32; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DIST_EXTRA: [u32; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/// zlib data (fixed-Huffman deflate with hash-chain matches): not zlib's
/// level-9 bytes, the same pixels.
pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut b = Bits { out: vec![0x78, 0xda], acc: 0, n: 0 };
    b.put(1, 1);
    b.put(1, 2);
    const WINDOW: usize = 32768;
    let mut head: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    let mut prev = vec![usize::MAX; data.len()];
    let key = |i: usize| (data[i] as u32) << 16 | (data[i + 1] as u32) << 8 | data[i + 2] as u32;
    let mut i = 0;
    while i < data.len() {
        let mut best = (0usize, 0usize);
        if i + 2 < data.len() {
            let k = key(i);
            let mut cand = head.get(&k).copied().unwrap_or(usize::MAX);
            let mut chain = 0;
            while cand != usize::MAX && i - cand <= WINDOW && chain < 64 {
                let mut l = 0;
                while l < 258 && i + l < data.len() && data[cand + l] == data[i + l] {
                    l += 1;
                }
                if l > best.0 {
                    best = (l, i - cand);
                }
                cand = prev[cand];
                chain += 1;
            }
        }
        let step = if best.0 >= 3 { best.0 } else { 1 };
        if best.0 >= 3 {
            let li = LEN_BASE.iter().rposition(|&x| x as usize <= best.0).unwrap_or(0);
            lit(&mut b, 257 + li as u32);
            b.put(best.0 as u32 - LEN_BASE[li], LEN_EXTRA[li]);
            let di = DIST_BASE.iter().rposition(|&x| x as usize <= best.1).unwrap_or(0);
            b.code(di as u32, 5);
            b.put(best.1 as u32 - DIST_BASE[di], DIST_EXTRA[di]);
        } else {
            lit(&mut b, data[i] as u32);
        }
        for j in i..i + step {
            if j + 2 < data.len() {
                let k = key(j);
                prev[j] = head.get(&k).copied().unwrap_or(usize::MAX);
                head.insert(k, j);
            }
        }
        i += step;
    }
    lit(&mut b, 256);
    let mut out = b.finish();
    out.extend(adler32(data).to_be_bytes());
    out
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    out.extend(kind);
    out.extend(data);
    let mut c = kind.to_vec();
    c.extend(data);
    out.extend(crc32(&c).to_be_bytes());
    out
}

/// `pngEncode(w, h, paint)`: an RGBA PNG, `paint(x, y)` each pixel.
pub fn png_encode(w: u32, h: u32, paint: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]);
    let mut raw = Vec::with_capacity((h * (1 + w * 4)) as usize);
    for y in 0..h {
        raw.push(0);
        for x in 0..w {
            raw.extend(paint(x, y));
        }
    }
    let mut out = vec![137, 80, 78, 71, 13, 10, 26, 10];
    out.extend(png_chunk(b"IHDR", &ihdr));
    out.extend(png_chunk(b"IDAT", &zlib(&raw)));
    out.extend(png_chunk(b"IEND", &[]));
    out
}

/// 3×5 glyphs for the tiles' numbers.
fn digit(c: char) -> Option<[u8; 5]> {
    Some(match c {
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [7, 1, 7, 4, 7],
        '3' => [7, 1, 7, 1, 7],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 7, 1, 7],
        '6' => [7, 4, 7, 5, 7],
        '7' => [7, 1, 1, 2, 2],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 7],
        '+' => [0, 2, 7, 2, 0],
        _ => return None,
    })
}

fn hex_rgb(s: &str) -> [f64; 3] {
    let h = |a: usize| u8::from_str_radix(&s[a..a + 2], 16).unwrap_or(0) as f64;
    [h(1), h(3), h(5)]
}

/// `mockTile(w, h, label, c0, c1)`: the number in white on a diagonal.
pub fn mock_tile(w: u32, h: u32, label: &str, c0: &str, c1: &str) -> Vec<u8> {
    let (a, b) = (hex_rgb(c0), hex_rgb(c1));
    let text: Vec<char> = label.chars().collect();
    let s = (w.min(h) as i64 / 9).max(2);
    let gw = text.len() as i64 * 4 * s - s;
    let gh = 5 * s;
    let gx = (w as i64 - gw).div_euclid(2);
    let gy = (h as i64 - gh).div_euclid(2);
    let lerp = |x: f64, y: f64, t: f64| ((x + (y - x) * t) + 0.5).floor().clamp(0.0, 255.0) as u8;
    png_encode(w, h, |x, y| {
        let dy = (y as i64 - gy).div_euclid(s);
        let k = (x as i64 - gx).div_euclid(s);
        let on = (0..=4).contains(&dy) && k >= 0 && k < text.len() as i64 * 4 - 1 && k % 4 != 3 && digit(text[(k / 4) as usize]).is_some_and(|g| g[dy as usize] & (4 >> (k % 4)) != 0);
        if on {
            return [255, 255, 255, 255];
        }
        let t = (x + y) as f64 / (w + h) as f64;
        [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t), 255]
    })
}

const MOCK_ASPECTS: [(f64, f64); 8] = [(16.0, 9.0), (1.0, 1.0), (4.0, 5.0), (3.0, 2.0), (9.0, 16.0), (4.0, 3.0), (3.0, 4.0), (2.0, 1.0)];
const MOCK_COLORS: [(&str, &str); 8] =
    [("#ef4444", "#b91c1c"), ("#3b82f6", "#1e40af"), ("#22c55e", "#15803d"), ("#f59e0b", "#b45309"), ("#8b5cf6", "#6d28d9"), ("#06b6d4", "#0e7490"), ("#ec4899", "#be185d"), ("#f97316", "#c2410c")];

/// `mockImages(dir, n, ar)`: n numbered tiles, at `ar` or cycling shapes.
pub fn mock_images(dir: &Path, n: usize, ar: Option<(f64, f64)>) -> std::io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let mut paths = Vec::new();
    for i in 0..n {
        let (aw, ah) = ar.unwrap_or(MOCK_ASPECTS[i % MOCK_ASPECTS.len()]);
        let long = 640.0;
        let round = |x: f64| (x + 0.5).floor() as u32;
        let (w, h) = if aw >= ah { (long as u32, round(long * ah / aw)) } else { (round(long * aw / ah), long as u32) };
        let p = dir.join(format!("mock-{}.png", i + 1));
        let (c0, c1) = MOCK_COLORS[i % MOCK_COLORS.len()];
        std::fs::write(&p, mock_tile(w, h, &(i + 1).to_string(), c0, c1))?;
        paths.push(p);
    }
    Ok(paths)
}

/// `mockFiles(dir, only)`: one file of every kind (`only`: those exts).
pub async fn mock_files(ctx: &HubCtx, src: &Path, dir: &Path, only: Option<&[String]>) -> Vec<PathBuf> {
    let _ = std::fs::create_dir_all(dir);
    let want = |e: &str| only.is_none_or(|o| o.iter().any(|x| x == e));
    let mut out: Vec<PathBuf> = Vec::new();
    let d = dir.to_string_lossy().into_owned();
    let at = |n: &str| dir.join(n);
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let put = |out: &mut Vec<PathBuf>, name: &str, data: &str| {
        if !want(&ext_of(name)) {
            return;
        }
        if std::fs::write(dir.join(name), data).is_ok() {
            out.push(dir.join(name));
        }
    };
    if want("png") {
        for (n, w, h, a, b) in
            [("sunset", 640, 400, "#f59e0b", "#ef4444"), ("ocean", 400, 600, "#06b6d4", "#3b82f6"), ("forest", 600, 600, "#22c55e", "#14532d"), ("night", 800, 450, "#6366f1", "#0f172a")]
        {
            let svg = at(&format!(".{n}.svg"));
            let body = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\"><defs><linearGradient id=\"g\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"><stop offset=\"0\" stop-color=\"{a}\"/><stop offset=\"1\" stop-color=\"{b}\"/></linearGradient></defs><rect width=\"{w}\" height=\"{h}\" fill=\"url(#g)\"/><text x=\"{}\" y=\"{}\" font-family=\"Helvetica\" font-size=\"36\" fill=\"#fff\" text-anchor=\"middle\">{n}</text></svg>",
                js::number(w as f64 / 2.0),
                js::number(h as f64 / 2.0 + 12.0)
            );
            let png = at(&format!("{n}.png"));
            let ok = std::fs::write(&svg, body).is_ok()
                && run(ctx, "qlmanage", &["-t", "-s", &(w.max(h)).to_string(), "-o", &d, &s(&svg)], Some(15000)).await.is_some()
                && run(ctx, "mv", &[&format!("{}.png", s(&svg)), &s(&png)], None).await.is_some()
                && run(ctx, "sips", &["-c", &h.to_string(), &w.to_string(), &s(&png)], None).await.is_some();
            if !ok {
                break;
            }
            out.push(png);
        }
    }
    if want("gif") || want("webp") {
        let mut frames: Vec<String> = Vec::new();
        for i in 0..20 {
            let x = 40 + (240.0 * ((i as f64 / 20.0) * std::f64::consts::PI).sin().abs() + 0.5).floor() as i64;
            for f in ["(", "-size", "320x180", "xc:#0f172a", "-fill"] {
                frames.push(f.into());
            }
            frames.push(format!("hsl({},80%,60%)", i * 18));
            frames.push("-draw".into());
            frames.push(format!("circle {x},110 {},110", x + 22));
            for f in ["-fill", "#ffffff", "-pointsize", "20", "-draw"] {
                frames.push(f.into());
            }
            frames.push(format!("text 16,34 'animated GIF {}/20'", i + 1));
            frames.push(")".into());
        }
        let gif = s(&at("bounce.gif"));
        let mut args: Vec<&str> = vec!["-delay", "6", "-loop", "0"];
        args.extend(frames.iter().map(String::as_str));
        args.push(&gif);
        if run(ctx, "magick", &args, Some(20000)).await.is_some() {
            if want("gif") {
                out.push(at("bounce.gif"));
            }
            if want("webp") && run(ctx, "magick", &[&gif, "-loop", "0", &s(&at("spin.webp"))], Some(20000)).await.is_some() {
                out.push(at("spin.webp"));
            }
        }
    }
    put(&mut out, "pulse.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"480\" height=\"270\" viewBox=\"0 0 480 270\"><defs><linearGradient id=\"g\" x1=\"0\" x2=\"1\"><stop offset=\"0\" stop-color=\"#6366f1\"/><stop offset=\"1\" stop-color=\"#ec4899\"/></linearGradient></defs><rect width=\"480\" height=\"270\" rx=\"20\" fill=\"url(#g)\"/><text x=\"32\" y=\"56\" font-family=\"Helvetica\" font-size=\"28\" font-weight=\"bold\" fill=\"#fff\">Animated SVG</text><circle cx=\"380\" cy=\"80\" r=\"30\" fill=\"#fde047\"><animate attributeName=\"r\" values=\"22;38;22\" dur=\"1.6s\" repeatCount=\"indefinite\"/></circle><polygon points=\"120,230 180,130 240,230\" fill=\"#ffffff99\"><animateTransform attributeName=\"transform\" type=\"rotate\" from=\"0 180 196\" to=\"360 180 196\" dur=\"4s\" repeatCount=\"indefinite\"/></polygon></svg>");
    put(&mut out, "contact.vcf", "BEGIN:VCARD\nVERSION:3.0\nFN:Kiffy Bot\nEND:VCARD\n");
    if want("png") {
        let svg = at(".chip.svg");
        let chip = at("chip.png");
        let ok = std::fs::write(&svg, "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"420\" height=\"210\"><rect width=\"420\" height=\"210\" rx=\"18\" fill=\"#2f7bf6\"/><text x=\"210\" y=\"122\" font-family=\"Helvetica\" font-size=\"34\" fill=\"#fff\" text-anchor=\"middle\">SVG SAMPLE</text></svg>").is_ok()
            && run(ctx, "qlmanage", &["-t", "-s", "420", "-o", &d, &s(&svg)], Some(15000)).await.is_some()
            && run(ctx, "mv", &[&format!("{}.png", s(&svg)), &s(&chip)], None).await.is_some()
            && run(ctx, "sips", &["-c", "210", "420", &s(&chip)], None).await.is_some();
        if ok {
            out.push(chip);
        }
    }
    if want("md") && std::fs::copy(src.join("fixtures").join("README.md"), at("README.md")).is_ok() {
        out.push(at("README.md"));
    }
    put(&mut out, "main.rs", "use std::collections::HashMap;\n\n/// Counts words in a sentence.\nfn count_words(text: &str) -> HashMap<&str, usize> {\n    let mut counts = HashMap::new();\n    for word in text.split_whitespace() {\n        *counts.entry(word).or_insert(0) += 1;\n    }\n    counts\n}\n\nfn main() {\n    let counts = count_words(\"the quick brown fox jumps over the lazy dog the end\");\n    let mut pairs: Vec<_> = counts.into_iter().collect();\n    pairs.sort_by(|a, b| b.1.cmp(&a.1));\n    for (word, n) in pairs.iter().take(3) {\n        println!(\"{word}: {n}\");\n    }\n}\n");
    put(&mut out, "App.tsx", "import { signal, Text, View } from \"nsh\";\n\nexport default function Counter() {\n  const n = signal(0);\n  return (\n    <View onPress={() => n.update((v) => v + 1)}>\n      <Text>{`Clicked ${n()} times`}</Text>\n    </View>\n  );\n}\n");
    put(&mut out, "data.json", r#"{"bot":"Kiffy","model":{"id":"gpt-6-luna","effort":"max","fast":false},"tags":["native","nsh"],"count":3,"ok":true}"#);
    put(&mut out, "budget.csv", "Category,Q1,Q2,Q3,Q4\nRent,3000,3000,3100,3100\nSoftware,420,460,510,505\nTravel,1200,300,900,1500\nMeals,260,310,280,330\n");
    put(&mut out, "notes.txt", "Meeting notes — Oct 19\n\n1. Ship file viewers\n2. Verify parity with Grok Bot\n3. Mock responses so tests cost nothing\n");
    if want("html") && std::fs::copy(src.join("fixtures").join("page.html"), at("page.html")).is_ok() {
        out.push(at("page.html"));
    }
    if want("pdf") {
        // the memo's source: put() writes it only when html is wanted, and
        // mockFiles takes it off the list again
        if want("html") {
            let _ = std::fs::write(at(".memo-src.html"), "<h1>Quarterly memo</h1><p>Revenue grew <b>18%</b> quarter over quarter.</p><ul><li>Launch the viewer</li><li>Hire two engineers</li></ul><table border=1><tr><th>Region</th><th>Sales</th></tr><tr><td>APAC</td><td>1.2M</td></tr><tr><td>EMEA</td><td>0.9M</td></tr></table>");
        }
        if std::fs::copy(src.join("fixtures").join("PDF_Elements_Showcase.pdf"), at("PDF_Elements_Showcase.pdf")).is_ok() {
            out.push(at("PDF_Elements_Showcase.pdf"));
        }
    }
    if want("m4a") || want("wav") || want("aiff") {
        let aiff = s(&at("voice.aiff"));
        if run(ctx, "say", &["-o", &aiff, "This is a mock audio file."], None).await.is_some() {
            if want("aiff") {
                out.push(at("voice.aiff"));
            }
            let m4a_ok = !want("m4a") || run(ctx, "afconvert", &["-f", "m4af", "-d", "aac", &aiff, &s(&at("voice.m4a"))], None).await.is_some();
            if want("m4a") && m4a_ok {
                out.push(at("voice.m4a"));
            }
            if m4a_ok && want("wav") && run(ctx, "afconvert", &["-f", "WAVE", "-d", "LEI16@22050", &aiff, &s(&at("tone.wav"))], None).await.is_some() {
                out.push(at("tone.wav"));
            }
        }
    }
    out
}

/// A stored (uncompressed) zip, as JSZip writes one by default.
fn zip_store(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let (mut out, mut central) = (Vec::new(), Vec::new());
    for (name, data) in files {
        let off = out.len() as u32;
        let crc = crc32(data);
        let head = |sig: u32, central: bool| {
            let mut h = sig.to_le_bytes().to_vec();
            if central {
                h.extend(20u16.to_le_bytes());
            }
            h.extend(20u16.to_le_bytes());
            h.extend(0u16.to_le_bytes());
            h.extend(0u16.to_le_bytes());
            h.extend(0u32.to_le_bytes());
            h.extend(crc.to_le_bytes());
            h.extend((data.len() as u32).to_le_bytes());
            h.extend((data.len() as u32).to_le_bytes());
            h.extend((name.len() as u16).to_le_bytes());
            h.extend(0u16.to_le_bytes());
            h
        };
        out.extend(head(0x0403_4b50, false));
        out.extend(name.as_bytes());
        out.extend(data);
        let mut c = head(0x0201_4b50, true);
        c.extend([0u8; 8]);
        c.extend(off.to_le_bytes());
        c.extend(name.as_bytes());
        central.extend(c);
    }
    let start = out.len() as u32;
    let n = files.len() as u16;
    out.extend(&central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend(n.to_le_bytes());
    out.extend(n.to_le_bytes());
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(start.to_le_bytes());
    out.extend([0u8; 2]);
    out
}

/// `mockDocx(dir)`: a real .docx (heading, bold run, list, table).
pub fn mock_docx(dir: &Path) -> std::io::Result<PathBuf> {
    let w = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;
    let p = |t: &str, style: Option<&str>| format!("<w:p>{}{t}</w:p>", style.map(|s| format!("<w:pPr><w:pStyle w:val=\"{s}\"/></w:pPr>")).unwrap_or_default());
    let r = |t: &str, b: bool| format!("<w:r>{}<w:t xml:space=\"preserve\">{t}</w:t></w:r>", if b { "<w:rPr><w:b/></w:rPr>" } else { "" });
    let cell = |t: &str| format!("<w:tc>{}</w:tc>", p(&r(t, false), None));
    let row = |a: &str, b: &str| format!("<w:tr>{}{}</w:tr>", cell(a), cell(b));
    let body = [
        p(&r("Quarterly memo", false), Some("Heading1")),
        p(&format!("{}{}{}", r("Revenue grew ", false), r("18%", true), r(" quarter over quarter.", false)), None),
        p(&r("Next steps", false), Some("Heading2")),
        p(&r("Launch the file viewers", false), Some("ListBullet")),
        p(&r("Hire two engineers", false), Some("ListBullet")),
        format!("<w:tbl>{}{}{}</w:tbl>", row("Region", "Sales"), row("APAC", "1.2M"), row("EMEA", "0.9M")),
        p(&r("Prepared by Kiffy.", false), None),
    ]
    .join("");
    let files = [
        ("[Content_Types].xml", r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#.as_bytes().to_vec()),
        ("_rels/.rels", r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.as_bytes().to_vec()),
        ("word/_rels/document.xml.rels", r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#.as_bytes().to_vec()),
        ("word/styles.xml", format!(r#"<?xml version="1.0"?><w:styles {w}><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/></w:style><w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/></w:style><w:style w:type="paragraph" w:styleId="ListBullet"><w:name w:val="List Bullet"/></w:style></w:styles>"#).into_bytes()),
        ("word/document.xml", format!(r#"<?xml version="1.0"?><w:document {w}><w:body>{body}</w:body></w:document>"#).into_bytes()),
    ];
    let out = dir.join("memo.docx");
    std::fs::write(&out, zip_store(&files))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deflate stream inflated back (stored and fixed-Huffman blocks):
    /// what the PNG decoders read.
    fn inflate(z: &[u8]) -> Vec<u8> {
        let d = &z[2..z.len() - 4];
        let mut pos = 0usize;
        let bit = |pos: &mut usize| -> u32 {
            let b = (d[*pos / 8] >> (*pos % 8)) & 1;
            *pos += 1;
            b as u32
        };
        let bits = |pos: &mut usize, n: u32| -> u32 { (0..n).fold(0, |acc, i| acc | (bit(pos) << i)) };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!((bit(&mut pos), bits(&mut pos, 2)), (1, 1), "one final fixed block");
        loop {
            // fixed literal/length code, read MSB first
            let mut code = 0u32;
            let mut len = 0;
            let sym = loop {
                code = (code << 1) | bit(&mut pos);
                len += 1;
                match len {
                    7 if code <= 0x17 => break code + 256,
                    8 if (0x30..=0xbf).contains(&code) => break code - 0x30,
                    8 if (0xc0..=0xc7).contains(&code) => break code - 0xc0 + 280,
                    9 if code >= 0x190 => break code - 0x190 + 144,
                    _ => {}
                }
            };
            match sym {
                0..=255 => out.push(sym as u8),
                256 => break,
                _ => {
                    let li = (sym - 257) as usize;
                    let l = LEN_BASE[li] + bits(&mut pos, LEN_EXTRA[li]);
                    let di = (0..5).fold(0u32, |acc, _| (acc << 1) | bit(&mut pos)) as usize;
                    let dist = DIST_BASE[di] + bits(&mut pos, DIST_EXTRA[di]);
                    for _ in 0..l {
                        out.push(out[out.len() - dist as usize]);
                    }
                }
            }
        }
        assert_eq!(adler32(&out).to_be_bytes(), z[z.len() - 4..]);
        out
    }

    #[test]
    fn files_png_deflate_round_trips() {
        let data: Vec<u8> = (0..20000u32).map(|i| ((i / 7) % 13 + (i % 3)) as u8).collect();
        assert_eq!(inflate(&zlib(&data)), data);
        let tile = mock_tile(64, 36, "12", "#ef4444", "#b91c1c");
        assert_eq!(&tile[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
    }

    #[test]
    fn files_kinds_and_variants_as_grok_reads_them() {
        assert_eq!((ext_of("a/B.PNG"), ext_of(".env"), ext_of("x.")), ("png".into(), "".into(), "".into()));
        assert_eq!((viewer_kind("a.md"), viewer_kind("a.csv"), viewer_kind("a.jsonc"), viewer_kind("a.zip")), ("markdown", "table", "text", "unknown"));
        assert_eq!((card_variant("a.md"), card_variant("a.mov"), card_variant("a.bin")), ("generic", "video", "unknown"));
        assert_eq!(view_box(r#"<svg viewBox="0 0 480 270">"#), Some((480.0, 270.0)));
        assert_eq!(view_box(r#"<svg viewBox="480 270">"#), Some((80.0, 270.0)));
    }
}
