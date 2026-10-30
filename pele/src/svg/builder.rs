// Port of src/svg/builder.ts. Math and icon spans are left out: with no resolver (see
// text/math.rs, text/icons.rs) a label never holds one.

use crate::js;
use crate::text::entities::decode_entities;
use crate::text::label::Label;
use crate::text::measurer::style;

fn dropped(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{7f}' | '\u{fffe}' | '\u{ffff}')
}

/// Whether a value holds a character that XML cannot (RE_INVALID_XML).
pub fn invalid_xml(text: &str) -> bool {
    text.chars().any(dropped)
}

/// Escapes text for XML and drops invalid XML characters and DEL.
pub fn esc(text: &str) -> String {
    if !text.chars().any(|c| matches!(c, '&' | '<' | '>' | '"') || dropped(c)) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if dropped(c) => {}
            c => out.push(c),
        }
    }
    out
}

/// Escapes text that is only ever read as text, entity codes turned into their characters first.
pub fn esc_text(text: &str) -> String {
    esc(&decode_entities(text))
}

/// A coordinate with at most two decimals, as upstream writes it.
pub fn num(value: f64) -> String {
    let n = js::round(value * 100.0);
    if n == 0.0 {
        return "0".into();
    }
    if n > 0.0 {
        if n < 1e11 {
            let q = (n / 100.0).floor();
            return format!("{}{}", q as i64, fraction((n - q * 100.0) as i64));
        }
    } else if n > -1e11 {
        let m = -n;
        let q = (m / 100.0).floor();
        return format!("-{}{}", q as i64, fraction((m - q * 100.0) as i64));
    }
    js::num_str(n / 100.0)
}

fn fraction(i: i64) -> String {
    if i == 0 {
        String::new()
    } else if i % 10 == 0 {
        format!(".{}", i / 10)
    } else if i < 10 {
        format!(".0{}", i)
    } else {
        format!(".{}", i)
    }
}

/// Writes a measured label as SVG text centered on (cx, cy).
pub fn label_svg(label: &Label, cx: f64, cy: f64, attrs: &str) -> String {
    let lines = &label.lines;
    if lines.is_empty() {
        return String::new();
    }
    let lh = label.line_height;
    let shift = label.size * 0.35;
    let mut top = cy - label.height / 2.0;

    if lines.len() == 1 && lines[0].len() == 1 && lines[0][0].style == 0 {
        return format!(
            "<text{} x=\"{}\" y=\"{}\" text-anchor=\"middle\">{}</text>",
            attrs,
            num(cx),
            num(top + lh / 2.0 + shift),
            esc(&lines[0][0].text)
        );
    }

    let shared = shared_style(label);
    let attrs = text_style(attrs, shared);
    let mut body = String::new();
    for line in lines {
        let h = lh;
        let mid = top + h / 2.0;
        let y = mid + shift;
        for (j, span) in line.iter().enumerate() {
            let pos = if j == 0 { format!(" x=\"{}\" y=\"{}\"", num(cx), num(y)) } else { String::new() };
            body.push_str(&format!("<tspan{}{}>{}</tspan>", pos, span_style(span.style & !shared), esc(&span.text)));
        }
        if line.is_empty() {
            body.push_str(&format!("<tspan x=\"{}\" y=\"{}\"> </tspan>", num(cx), num(y)));
        }
        top += h;
    }
    format!("<text{} text-anchor=\"middle\" xml:space=\"preserve\">{}</text>", attrs, body)
}

const BOLD: &str = " font-weight=\"var(--_w)\"";
const ITALIC: &str = " font-style=\"italic\"";
const UNDERLINE: &str = " text-decoration=\"underline\"";
const MONO: &str = " font-family=\"var(--_fm)\"";
const SUB: &str = " baseline-shift=\"sub\" font-size=\"smaller\"";
const SUP: &str = " baseline-shift=\"super\" font-size=\"smaller\"";

pub fn span_style(s: u32) -> String {
    let mut out = String::new();
    if s & style::BOLD != 0 {
        out.push_str(BOLD);
    }
    if s & style::ITALIC != 0 {
        out.push_str(ITALIC);
    }
    if s & style::MONO != 0 {
        out.push_str(MONO);
    }
    if s & style::UNDERLINE != 0 {
        out.push_str(UNDERLINE);
    }
    if s & style::SUB != 0 {
        out.push_str(SUB);
    } else if s & style::SUP != 0 {
        out.push_str(SUP);
    }
    out
}

// The emphasis every span of a label has, written once on the text element.
pub fn shared_style(label: &Label) -> u32 {
    let mut shared = style::BOLD | style::ITALIC | style::MONO;
    let mut any = false;
    for line in &label.lines {
        for span in line {
            if span.text.is_empty() {
                continue;
            }
            shared &= span.style;
            any = true;
        }
    }
    if any {
        shared
    } else {
        0
    }
}

// Adds that emphasis to a text element's attributes, unless they give a weight of their own.
pub fn text_style(attrs: &str, shared: u32) -> String {
    let mut out = attrs.to_string();
    if shared & style::BOLD != 0 && !attrs.contains(" font-weight=\"") {
        out.push_str(BOLD);
    }
    if shared & style::ITALIC != 0 && !attrs.contains(" font-style=\"") {
        out.push_str(ITALIC);
    }
    if shared & style::MONO != 0 && !attrs.contains(" font-family=\"") {
        out.push_str(MONO);
    }
    out
}
