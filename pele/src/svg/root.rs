// Port of src/svg/root.ts.

use super::builder::{esc, esc_text};
use super::theme::{font_style, RADIUS, ROOT_STYLE};
use crate::js;

// The weights of bold text, each set only in a diagram that has such text.
const WEIGHTS: [(&str, &str); 3] = [("tw", "title"), ("hw", "heading"), ("w", "bold")];

#[derive(Default)]
pub struct Accessible<'a> {
    pub acc_title: Option<&'a str>,
    pub acc_descr: Option<&'a str>,
}

/// The aliases (`r`, `tw`, `hw`, `w`) that `var(--_…)` uses in the content, as RE_USED finds them.
fn used(content: &str) -> Vec<&'static str> {
    let mut found: Vec<&'static str> = Vec::new();
    let mut from = 0;
    while let Some(p) = content[from..].find("var(--_") {
        let at = from + p + 7;
        let rest = &content[at..];
        for name in ["r", "tw", "hw", "w"] {
            if rest.len() > name.len() && rest.starts_with(name) && rest.as_bytes()[name.len()] == b')' {
                if !found.contains(&name) {
                    found.push(name);
                }
                break;
            }
        }
        if found.len() == 4 {
            break;
        }
        from = at;
    }
    found
}

pub fn svg_document(type_: &str, width: f64, height: f64, font_size: f64, responsive: bool, acc: &Accessible, content: &str) -> String {
    let prefix = "pele";
    let mut head = String::new();
    let mut aria = String::new();
    if let Some(t) = acc.acc_title.filter(|t| !t.is_empty()) {
        head.push_str(&format!("<title id=\"{}-title\">{}</title>", prefix, esc_text(t)));
        aria.push_str(&format!(" aria-labelledby=\"{}-title\"", prefix));
    }
    if let Some(d) = acc.acc_descr.filter(|d| !d.is_empty()) {
        head.push_str(&format!("<desc id=\"{}-desc\">{}</desc>", prefix, esc_text(d)));
        aria.push_str(&format!(" aria-describedby=\"{}-desc\"", prefix));
    }
    let fit = if responsive { "max-width:100%;height:auto;" } else { "" };
    let used = used(content);
    let mut weights = if used.contains(&"r") { format!(";--_r:{}", RADIUS) } else { String::new() };
    for (alias, name) in WEIGHTS {
        if used.contains(&alias) {
            weights.push_str(&format!(";--_{}:var(--pele-{}-weight,bold)", alias, name));
        }
    }
    let (w, h) = (js::num_str(width), js::num_str(height));
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" class=\"pele pele-{t}\" viewBox=\"0 0 {w} {h}\" width=\"{w}\" height=\"{h}\" style=\"{fit}{root}{weights}{font}\" fill=\"var(--_fg)\" role=\"graphics-document document\" aria-roledescription=\"{t}\"{aria}>{head}{content}</svg>",
        t = type_,
        root = ROOT_STYLE,
        font = esc(&font_style(font_size)),
    )
}
