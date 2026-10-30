// Port of src/svg/theme.ts.

use super::builder::{esc, invalid_xml};
use crate::js;

// Private aliases set once on the root element, so each use is short and the public --pele-*
// tokens keep their fallbacks.
pub const ROOT_STYLE: &str = "--_bg:var(--pele-bg,#fff);--_fg:var(--pele-fg,#1f1f1f);--_m:var(--pele-muted,#6e6e6e);\
--_l:var(--pele-line,#1f1f1f);--_s:var(--pele-surface,#f3f3f3);--_a:var(--pele-surface-alt,#e6e6e6);\
--_b:var(--pele-border,#8a8a8a);--_c:var(--pele-accent,#5b7bd5);color:var(--_fg)";

pub const FONT_MONO: &str = "var(--_fm)";
pub const RADIUS: &str = "var(--pele-radius,4px)";

// Use CSS for rx variables to support older browsers.
pub const ROUNDED: &str = " style=\"rx:var(--_r)\"";

pub const CRITICAL: &str = "var(--pele-critical,#e45756)";

// Expects a shape style from resolve_style().
pub fn sets_fill(shape: &str) -> bool {
    shape.contains("\"fill:") || shape.contains(";fill:")
}

pub fn rounded(attrs: &str) -> String {
    match attrs.find(" style=\"") {
        None => format!("{}{}", attrs, ROUNDED),
        Some(at) => format!("{}rx:var(--_r);{}", &attrs[..at + 8], &attrs[at + 8..]),
    }
}

// The fonts the labels were measured in are the ones they are drawn in, unless the page names
// others. The host passes no families, so these are the generic ones.
pub fn font_style(size: f64) -> String {
    format!(
        ";--_fm:var(--pele-font-mono,monospace);font:{}px sans-serif;letter-spacing:normal;word-spacing:normal;text-transform:none;direction:ltr;font-family:var(--pele-font,sans-serif)",
        js::num_str(size)
    )
}

const SHAPE_PROPS: [&str; 12] = ["fill", "fill-opacity", "stroke", "stroke-width", "stroke-dasharray", "stroke-dashoffset", "stroke-linecap", "stroke-linejoin", "stroke-opacity", "opacity", "rx", "ry"];
const TEXT_PROPS: [&str; 9] = ["color", "font-size", "font-family", "font-weight", "font-style", "text-decoration", "letter-spacing", "word-spacing", "opacity"];

fn unsafe_value(value: &str) -> bool {
    let v = value.to_lowercase();
    let call = |word: &str| {
        let mut from = 0;
        while let Some(p) = v[from..].find(word) {
            let after = js::trim_start(&v[from + p + word.len()..]);
            if after.starts_with('(') {
                return true;
            }
            from += p + 1;
        }
        false
    };
    call("url")
        || call("expression")
        || v.contains("javascript:")
        || v.contains(['<', '>', '{', '}', ';', '\\'])
        || v.contains("@import")
        || v.contains("image-set")
        || v.contains("/*")
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedStyle {
    pub shape: String,
    pub line: String,
    pub text: String,
    pub stroke: Option<String>,
    pub bold: bool,
    pub italic: bool,
    pub font_size: Option<f64>,
}

fn set(map: &mut Vec<(String, String)>, key: &str, value: &str) {
    match map.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = value.to_string(),
        None => map.push((key.to_string(), value.to_string())),
    }
}

fn get<'a>(map: &'a [(String, String)], key: &str) -> Option<&'a str> {
    map.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn remove_important(s: &str) -> String {
    if !s.to_ascii_lowercase().contains("!important") {
        return s.to_string();
    }
    let mut out = String::new();
    let mut i = 0;
    let b = s.as_bytes();
    while i < b.len() {
        if b[i] == b'!' && i + 10 <= b.len() && b[i..i + 10].eq_ignore_ascii_case(b"!important") {
            i += 10;
            continue;
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

// Splits Mermaid style declarations into what applies to a shape and what applies to its text.
pub fn resolve_style(declarations: &[String]) -> ResolvedStyle {
    if declarations.is_empty() {
        return ResolvedStyle::default();
    }
    let mut shape: Vec<(String, String)> = Vec::new();
    let mut text: Vec<(String, String)> = Vec::new();
    for decl in declarations {
        let Some(at) = decl.find(':') else { continue };
        if at == 0 {
            continue;
        }
        let prop = js::trim(&decl[..at]).to_lowercase();
        let value = remove_important(&decl[at + 1..]);
        let value = js::trim(&value);
        if value.is_empty() || unsafe_value(value) || invalid_xml(value) {
            continue;
        }
        if SHAPE_PROPS.contains(&prop.as_str()) {
            set(&mut shape, &prop, value);
        }
        if TEXT_PROPS.contains(&prop.as_str()) {
            set(&mut text, if prop == "color" { "fill" } else { &prop }, value);
        }
    }
    if shape.is_empty() && text.is_empty() {
        return ResolvedStyle::default();
    }
    let mut shape_css = String::new();
    let mut line_css = String::new();
    for (prop, value) in &shape {
        shape_css.push_str(&format!("{}:{};", prop, value));
        if prop != "fill" && prop != "fill-opacity" {
            line_css.push_str(&format!("{}:{};", prop, value));
        }
    }
    let mut text_css = String::new();
    for (prop, value) in &text {
        text_css.push_str(&format!("{}:{};", prop, value));
    }
    let weight = get(&text, "font-weight");
    let size = get(&text, "font-size");
    let px = size.filter(|s| is_px(s)).map(js::parse_float);
    let wrap = |css: &str| if css.is_empty() { String::new() } else { format!(" style=\"{}\"", esc(css)) };
    ResolvedStyle {
        shape: wrap(&shape_css),
        line: wrap(&line_css),
        text: wrap(&text_css),
        stroke: get(&shape, "stroke").map(str::to_string),
        bold: matches!(weight, Some("bold" | "bolder")) || weight.is_some_and(|w| js::to_number(w) >= 600.0),
        italic: get(&text, "font-style") == Some("italic"),
        font_size: px.filter(|&p| p > 0.0 && p <= 200.0),
    }
}

// /^[\d.]+(px)?$/
fn is_px(s: &str) -> bool {
    let body = s.strip_suffix("px").unwrap_or(s);
    !body.is_empty() && body.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

pub fn class_names(raw: &str) -> String {
    let mut cleaned = String::new();
    let mut gap = false;
    for c in raw.chars() {
        if js::is_word(c) || c == '-' {
            cleaned.push(c);
            gap = false;
        } else if !gap {
            cleaned.push(' ');
            gap = true;
        }
    }
    let cleaned = js::trim(&cleaned);
    if cleaned.is_empty() {
        String::new()
    } else {
        format!(" {}", cleaned)
    }
}

const SERIES: [&str; 8] = ["#4c78a8", "#f58518", "#54a24b", "#e45756", "#72b7b2", "#eeca3b", "#b279a2", "#9d755d"];

// Color for the nth data series in a chart, cycling through eight tokens.
pub fn series_color(index: i64) -> String {
    let k = ((index % 8) + 8) % 8;
    format!("var(--pele-series-{},{})", k + 1, SERIES[k as usize])
}
