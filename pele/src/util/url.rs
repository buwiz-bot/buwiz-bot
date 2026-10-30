// Port of src/util/url.ts. `new URL()` is the url crate, which follows the same WHATWG spec.

use crate::js;
use crate::svg::builder::{esc, invalid_xml};
use crate::text::entities::decode_entities;
use crate::types::LinkInfo;

const BLANK: &str = "about:blank";

fn is_ctrl(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{2000}'..='\u{200d}' | '\u{feff}' | '\u{fffe}' | '\u{ffff}')
}

fn strip_ctrl(s: &str) -> String {
    s.chars().filter(|&c| !is_ctrl(c)).collect()
}

fn word_len(b: &[u8], from: usize) -> usize {
    b[from.min(b.len())..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == b'_').count()
}

/// .replace(/&#(\w+)(^\w|;)?/g, (_m, dec) => String.fromCharCode(dec))
fn html_entities(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::new();
    let mut last = 0;
    let mut i = 0;
    while i + 2 < b.len() + 1 && i < b.len() {
        if b[i] == b'&' && b.get(i + 1) == Some(&b'#') {
            let w = word_len(b, i + 2);
            if w > 0 {
                let dec = &s[i + 2..i + 2 + w];
                let mut end = i + 2 + w;
                if b.get(end) == Some(&b';') {
                    end += 1;
                }
                let n = js::to_number(dec);
                let code = if n.is_finite() { (n.trunc().rem_euclid(65536.0)) as u32 } else { 0 };
                out.push_str(&s[last..i]);
                out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                last = end;
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&s[last..]);
    out
}

fn has_html_entity(s: &str) -> bool {
    let b = s.as_bytes();
    (0..b.len()).any(|i| b[i] == b'&' && b.get(i + 1) == Some(&b'#') && word_len(b, i + 2) > 0)
}

/// .replace(/&(newline|tab);/gi, '')
fn ctrl_entities(s: &str) -> (String, bool) {
    let lower = s.to_ascii_lowercase();
    let mut out = String::new();
    let mut i = 0;
    let mut found = false;
    while i < s.len() {
        if lower[i..].starts_with("&newline;") {
            i += 9;
            found = true;
        } else if lower[i..].starts_with("&tab;") {
            i += 5;
            found = true;
        } else {
            let c = s[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    (out, found)
}

/// .replace(/(\\|%5[cC])((%(6[eE]|72|74))|[nrt])/g, '')
fn ws_escapes(s: &str) -> (String, bool) {
    let b = s.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    let mut found = false;
    let mut last = 0;
    while i < b.len() {
        let lead = if b[i] == b'\\' {
            1
        } else if b[i] == b'%' && b.get(i + 1) == Some(&b'5') && matches!(b.get(i + 2), Some(b'c' | b'C')) {
            3
        } else {
            0
        };
        if lead > 0 {
            let j = i + lead;
            let tail = if b.get(j) == Some(&b'%') && ((b.get(j + 1) == Some(&b'6') && matches!(b.get(j + 2), Some(b'e' | b'E'))) || (b.get(j + 1) == Some(&b'7') && matches!(b.get(j + 2), Some(b'2' | b'4')))) {
                3
            } else if matches!(b.get(j), Some(b'n' | b'r' | b't')) {
                1
            } else {
                0
            };
            if tail > 0 {
                out.push_str(&s[last..i]);
                i = j + tail;
                last = i;
                found = true;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&s[last..]);
    (out, found)
}

/// decodeURIComponent, or None where it throws.
fn decode_component(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let hex = |i: usize| -> Option<u8> {
        let h = b.get(i..i + 2)?;
        u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok().filter(|_| h.iter().all(|c| c.is_ascii_hexdigit()))
    };
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'%' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        let first = hex(i + 1)?;
        i += 3;
        let more = match first {
            0x00..=0x7f => 0,
            0xc0..=0xdf => 1,
            0xe0..=0xef => 2,
            0xf0..=0xf7 => 3,
            _ => return None,
        };
        let mut seq = vec![first];
        for _ in 0..more {
            if b.get(i) != Some(&b'%') {
                return None;
            }
            let c = hex(i + 1)?;
            if c & 0xc0 != 0x80 {
                return None;
            }
            seq.push(c);
            i += 3;
        }
        std::str::from_utf8(&seq).ok()?;
        out.extend(seq);
    }
    String::from_utf8(out).ok()
}

fn decode(uri: &str) -> String {
    decode_component(uri).unwrap_or_else(|| uri.to_string())
}

// Removes the control characters, entity codes and escapes that could hide a scheme.
fn unwrap(url: &str, percent: bool) -> Option<String> {
    let mut out = if percent { decode(js::trim(url)) } else { js::trim(url).to_string() };
    let mut rounds = 0;
    loop {
        rounds += 1;
        if rounds > 32 {
            return None;
        }
        out = strip_ctrl(&out);
        out = html_entities(&out);
        out = ctrl_entities(&out).0;
        out = strip_ctrl(&out);
        out = js::trim(&ws_escapes(&out).0).to_string();
        if percent {
            out = decode(&out);
        }
        let more = out.chars().any(is_ctrl) || has_html_entity(&out) || ctrl_entities(&out).1 || ws_escapes(&out).1;
        if !more {
            break;
        }
    }
    Some(out)
}

/// RE_SCHEME /^.+(:|&colon;)/gim: the first line with a colon, up to its last one.
fn scheme_of(s: &str) -> Option<String> {
    let mut line_start = 0;
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut k = 0;
    while k <= chars.len() {
        // Find this line's end.
        let mut e = k;
        while e < chars.len() && !matches!(chars[e].1, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
            e += 1;
        }
        let line_byte_end = if e < chars.len() { chars[e].0 } else { s.len() };
        let line = &s[line_start..line_byte_end];
        // Rightmost ':' or '&colon;' that starts after the first character.
        let mut best: Option<usize> = None;
        let lb: Vec<(usize, char)> = line.char_indices().collect();
        for (idx, &(bi, c)) in lb.iter().enumerate() {
            if idx == 0 {
                continue;
            }
            if c == ':' {
                best = Some(bi + 1);
            } else if line[bi..].starts_with("&colon;") {
                best = Some(bi + 7);
            }
        }
        if let Some(end) = best {
            return Some(line[..end].to_string());
        }
        if e >= chars.len() {
            break;
        }
        k = e + 1;
        line_start = if k < chars.len() { chars[k].0 } else { s.len() };
    }
    None
}

fn invalid_protocol(protocol: &str) -> bool {
    let rest = protocol.trim_start_matches(|c: char| !js::is_word(c));
    rest.starts_with("javascript") || rest.starts_with("data") || rest.starts_with("vbscript")
}

// Refuses what @braintree/sanitize-url refuses. Unlike it, keeps percent escapes.
pub fn sanitize_url(url: &str) -> String {
    if url.is_empty() {
        return BLANK.into();
    }
    let decoded = unwrap(url, true);
    let kept = unwrap(url, false);
    let (Some(decoded), Some(kept)) = (decoded.filter(|d| !d.is_empty()), kept.filter(|k| !k.is_empty())) else { return BLANK.into() };
    if decoded.starts_with('.') || decoded.starts_with('/') {
        return kept;
    }
    let Some(scheme) = scheme_of(js::trim_start(&decoded)) else { return kept };
    let protocol = js::trim(&scheme.to_lowercase()).to_string();
    if invalid_protocol(&protocol) {
        return BLANK.into();
    }
    let normalized = js::trim_start(&kept).replace('\\', "/");
    if protocol == "mailto:" || protocol.contains("://") {
        return normalized;
    }
    if protocol == "http:" || protocol == "https:" {
        return match url::Url::parse(&normalized) {
            Ok(u) => u.as_str().to_string(),
            Err(_) => BLANK.into(),
        };
    }
    normalized
}

// A URL as written in a diagram: entity codes are read first, so they cannot hide a scheme.
pub fn safe_url(url: &str) -> String {
    sanitize_url(&decode_entities(url))
}

const LINK_SCHEMES: [&str; 4] = ["http", "https", "mailto", "tel"];
const IMAGE_SCHEMES: [&str; 2] = ["http", "https"];

// Stricter than Mermaid: a scheme must be one the host allows.
fn allowed(url: &str, schemes: &[&str]) -> bool {
    let b = url.as_bytes();
    if b.len() >= 2 && (b[0] == b'\\' || b[0] == b'/') && (b[1] == b'\\' || b[1] == b'/') {
        return false;
    }
    // /^([a-z][a-z0-9+.-]*):/i
    if b.first().is_some_and(|c| c.is_ascii_alphabetic()) {
        let n = b.iter().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'.' | b'-')).count();
        if b.get(n) == Some(&b':') {
            return schemes.contains(&url[..n].to_ascii_lowercase().as_str());
        }
    }
    true
}

fn allow(url: &str, schemes: &[&str]) -> Option<String> {
    let decoded = unwrap(url, true)?;
    if allowed(url, schemes) && allowed(&decoded, schemes) {
        Some(url.to_string())
    } else {
        None
    }
}

pub fn link_url(url: &str) -> Option<String> {
    allow(url, &LINK_SCHEMES)
}

pub fn image_url(url: &str) -> Option<String> {
    allow(url, &IMAGE_SCHEMES)
}

// The href keeps the original name, so sanitizing alone is not enough.
pub fn internal_link(name: &str) -> bool {
    let url = sanitize_url(name);
    if url == BLANK || link_url(&url).is_none() {
        return false;
    }
    // Match character removal by esc() and the browser.
    let seen: String = name.chars().filter(|&c| !invalid_xml(&c.to_string()) && c != '\t' && c != '\n' && c != '\r').collect();
    let seen = seen.trim_start_matches(|c: char| c <= '\u{20}');
    link_url(seen).is_some()
}

pub fn linked(body: &str, url: &str, id: &str, links: &mut Vec<LinkInfo>, attrs: &str) -> String {
    let Some(href) = link_url(url) else { return body.to_string() };
    let out = format!("<a href=\"{}\"{}{}>{}</a>", esc(&href), attrs, rel_attr(), body);
    links.push(LinkInfo { id: id.to_string(), href, internal: false });
    out
}

pub fn rel_attr() -> &'static str {
    " rel=\"noopener\""
}
