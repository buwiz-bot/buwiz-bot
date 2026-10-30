// Port of src/text/label.ts. Labels are read as chars where upstream reads UTF-16 units; the
// two only differ in how far an index moves past a character outside the BMP, and every
// decision here is on characters inside it. Math and icon spans are left out (see math.rs,
// icons.rs): with no resolver, `$$…$$` stays text and an icon name leaves no gap.

use super::entities::decode_entities;
use super::icons::icon_markup;
use super::measurer::{style, width as measure};
use crate::js;

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub text: String,
    pub style: u32,
    pub width: f64,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Label {
    pub lines: Vec<Vec<Span>>,
    pub widths: Vec<f64>,
    pub width: f64,
    pub height: f64,
    pub size: f64,
    pub line_height: f64,
}

pub const LINE_HEIGHT: f64 = 1.5;
// Lines of one wrapped label sit closer, so the label reads as one block.
const WRAPPED_LINE_HEIGHT: f64 = 1.3;

fn tag_style(tag: &str) -> u32 {
    match tag {
        "b" | "strong" => style::BOLD,
        "i" | "em" | "cite" | "dfn" | "var" => style::ITALIC,
        "code" | "kbd" | "samp" | "tt" => style::MONO,
        "sub" => style::SUB,
        "sup" => style::SUP,
        "u" | "ins" => style::UNDERLINE,
        _ => 0,
    }
}

const SHIFT: u32 = style::SUB | style::SUP;

const HTML_TAGS: &str = "a abbr address article aside audio b bdi bdo big blockquote body br button caption center cite code col \
colgroup data dd del details dfn dialog div dl dt em embed fieldset figcaption figure font footer form h1 h2 \
h3 h4 h5 h6 head header hr html i iframe img input ins kbd label legend li link main map mark meta meter nav \
noscript object ol optgroup option output p picture pre progress q rp rt ruby s samp script section select \
small source span strike strong style sub summary sup table tbody td template textarea tfoot th thead time \
title tr track tt u ul var video wbr";

fn is_html_tag(tag: &str) -> bool {
    HTML_TAGS.split(' ').any(|t| t == tag)
}

// Break between any two Chinese or Japanese characters. Hangul is left out.
fn is_cjk(c: char) -> bool {
    matches!(c, '\u{2e80}'..='\u{9fff}' | '\u{f900}'..='\u{faff}' | '\u{ff00}'..='\u{ff60}')
}
const CLOSE: &str = "、。，．：；！？）］｝」』】〕〉》〙〗ー…‥・ゝゞぁぃぅぇぉっゃゅょゎァィゥェォッャュョヮ,.:;!?)]}";
const OPEN: &str = "（［｛「『【〔〈《〘〖([{";
fn is_close(c: char) -> bool {
    CLOSE.contains(c)
}
fn is_open(c: char) -> bool {
    OPEN.contains(c)
}
// Break hints: a zero-width space, and a soft hyphen, which shows as '-' where a line breaks.
fn is_hint(c: char) -> bool {
    c == '\u{200b}' || c == '\u{ad}'
}

/// `text.match(RE_PIECE)`: runs of one word or CJK character with the spaces around them.
fn pieces(text: &str) -> Vec<String> {
    let c: Vec<char> = text.chars().collect();
    let n = c.len();
    let sp = |i: usize| i < n && js::is_space(c[i]);
    let mut out = Vec::new();
    let mut p = 0;
    while p < n {
        let mut q = p;
        while sp(q) {
            q += 1;
        }
        // The group after the leading spaces, if one starts there.
        let mut end: Option<usize> = None;
        if q < n {
            let mut k = 0;
            while q + k < n && is_open(c[q + k]) {
                k += 1;
            }
            let mut j = k as isize;
            while j >= 0 {
                let at = q + j as usize;
                if at < n && is_cjk(c[at]) {
                    let mut e = at + 1;
                    while e < n && is_close(c[e]) {
                        e += 1;
                    }
                    end = Some(e);
                    break;
                }
                j -= 1;
            }
            if end.is_none() && !js::is_space(c[q]) && !is_cjk(c[q]) && !is_hint(c[q]) {
                let mut e = q;
                while e < n && !js::is_space(c[e]) && !is_cjk(c[e]) && !is_hint(c[e]) {
                    e += 1;
                }
                while e < n && is_hint(c[e]) {
                    e += 1;
                }
                while e < n && is_close(c[e]) {
                    e += 1;
                }
                end = Some(e);
            }
            if end.is_none() && is_hint(c[q]) {
                let mut e = q;
                while e < n && is_hint(c[e]) {
                    e += 1;
                }
                end = Some(e);
            }
        }
        let stop = match end {
            Some(mut e) => {
                while sp(e) {
                    e += 1;
                }
                e
            }
            None if q > p => q,
            // Unreachable: every character starts one of the alternatives.
            None => p + 1,
        };
        out.push(c[p..stop].iter().collect());
        p = stop;
    }
    out
}

fn dedent(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut min = usize::MAX;
    for line in &lines {
        if js::trim(line).is_empty() {
            continue;
        }
        let indent = line.chars().count() - js::trim_start(line).chars().count();
        min = min.min(indent);
    }
    if min == usize::MAX || min == 0 {
        return text.to_string();
    }
    lines
        .iter()
        .map(|line| {
            let lead = line.chars().count() - js::trim_start(line).chars().count();
            line.chars().skip(min.min(lead)).collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn push_plain(line: &mut Vec<Span>, text: &str, st: u32) {
    let decoded = decode_entities(text);
    if let Some(last) = line.last_mut() {
        if last.style == st {
            last.text.push_str(&decoded);
            return;
        }
    }
    line.push(Span { text: decoded, style: st, width: 0.0 });
}

fn push_text(line: &mut Vec<Span>, text: &str, st: u32) {
    if text.is_empty() {
        return;
    }
    push_plain(line, text, st);
}

/// Finds `(fa[bklrs]?):fa-([\w-]+)` from byte `from`; returns (start, end, icon name).
fn find_icon(text: &str, from: usize) -> Option<(usize, usize, String)> {
    let b = text.as_bytes();
    let mut i = from;
    while i + 1 < b.len() {
        if b[i] == b'f' && b[i + 1] == b'a' {
            let mut p = i + 2;
            if p < b.len() && matches!(b[p], b'b' | b'k' | b'l' | b'r' | b's') && b.get(p + 1) == Some(&b':') {
                p += 1;
            }
            if text[p..].starts_with(":fa-") {
                let s = p + 4;
                let w = b[s..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == b'_' || **c == b'-').count();
                if w > 0 {
                    return Some((i, s + w, format!("{}:fa-{}", &text[i..p], &text[s..s + w])));
                }
            }
        }
        i += 1;
    }
    None
}

fn push_icons(line: &mut Vec<Span>, text: &str, st: u32) {
    if !text.contains(":fa-") {
        push_text(line, text, st);
        return;
    }
    let mut last = 0;
    let mut from = 0;
    while let Some((start, end, icon)) = find_icon(text, from) {
        push_text(line, &text[last..start], st);
        last = end;
        from = end;
        if !icon_markup(&icon).is_empty() {
            unreachable!("no icon resolver");
        } else if text[last..].starts_with(' ') {
            // An icon the host does not have leaves no gap, nor the space that set it off from the text.
            last += 1;
        } else if last == text.len() {
            if let Some(before) = line.last_mut() {
                before.text = js::trim_end(&before.text).to_string();
            }
        }
    }
    push_text(line, &text[last..], st);
}

struct Open {
    style: u32,
    // Sub/sup stack; only the innermost tag shifts text.
    shifts: Vec<u32>,
}

/// Matches RE_TAG at `i`: returns (tag name, length in chars).
fn match_tag(c: &[char], i: usize) -> Option<(String, usize)> {
    let n = c.len();
    let mut p = i + 1;
    if p < n && c[p] == '/' {
        p += 1;
    }
    if p >= n || !c[p].is_ascii_alphabetic() {
        return None;
    }
    let name_start = p;
    p += 1;
    while p < n && (js::is_word(c[p]) || c[p] == '-') {
        p += 1;
    }
    let name: String = c[name_start..p].iter().collect();
    if p < n && js::is_space(c[p]) {
        let mut q = p + 1;
        while q < n && c[q] != '<' && c[q] != '>' {
            q += 1;
        }
        if q < n && c[q] == '>' {
            return Some((name, q + 1 - i));
        }
        return None;
    }
    if p < n && c[p] == '/' {
        p += 1;
    }
    if p < n && c[p] == '>' {
        return Some((name, p + 1 - i));
    }
    None
}

// Splits inline HTML and, for markdown labels, emphasis markers into styled spans.
fn inline(line: &mut Vec<Span>, source: &str, markdown: bool, base: u32, open: &mut Open) {
    let c: Vec<char> = source.chars().collect();
    let n = c.len();
    let mut st = open.style;
    let mut marked = 0u32;
    let mut text = String::new();
    let mut i = 0;
    let flush = |line: &mut Vec<Span>, text: &mut String, st: u32, open: &Open| {
        push_icons(line, text, st | open.shifts.last().copied().unwrap_or(0));
        text.clear();
    };
    while i < n {
        let ch = c[i];
        if ch == '<' {
            if let Some((tag, len)) = match_tag(&c, i) {
                let tag = tag.to_lowercase();
                let closing = c.get(i + 1) == Some(&'/');
                let bit = tag_style(&tag);
                if bit & SHIFT != 0 {
                    flush(line, &mut text, st, open);
                    let at = open.shifts.iter().rposition(|&b| b == bit);
                    if !closing {
                        open.shifts.push(bit);
                    } else if let Some(at) = at {
                        open.shifts.remove(at);
                    }
                } else if bit != 0 {
                    flush(line, &mut text, st, open);
                    open.style = if closing { (open.style & !bit) | (base & bit) } else { open.style | bit };
                    st = if closing { (st & !bit) | (base & bit) } else { st | bit };
                }
                // Other HTML tags are dropped and their text kept. Anything else shaped like a
                // tag, such as the type in List<T>, is text.
                if bit != 0 || is_html_tag(&tag) {
                    i += len;
                    continue;
                }
            }
        } else if markdown && (ch == '*' || ch == '_') {
            let double = c.get(i + 1) == Some(&ch);
            let bit = if double { style::BOLD } else { style::ITALIC };
            let len = if double { 2 } else { 1 };
            let opening = marked & bit == 0;
            let prev = if i > 0 { c[i - 1] } else { ' ' };
            let next = if i + len < n { c[i + len] } else { ' ' };
            let rest: String = c[(i + len).min(n)..].iter().collect();
            let needle: String = if double { [ch, ch].iter().collect() } else { ch.to_string() };
            let can_open = opening && !js::is_space(next) && rest.contains(&needle);
            let can_close = !opening && !js::is_space(prev);
            let wordy = ch == '_' && js::is_word(prev) && js::is_word(next);
            if (can_open || can_close) && !wordy {
                flush(line, &mut text, st, open);
                marked = if can_open { marked | bit } else { marked & !bit };
                st = if can_open { st | bit } else { (st & !bit) | (base & bit) };
                i += len;
                continue;
            }
        }
        text.push(ch);
        i += 1;
    }
    flush(line, &mut text, st, open);
}

fn measure_spans(line: &mut [Span], size: f64) -> f64 {
    let mut total = 0.0;
    for span in line.iter_mut() {
        span.width = measure(&span.text, size, span.style);
        total += span.width;
    }
    total
}

fn wrap(line: &[Span], size: f64, max: f64, out: &mut Vec<Vec<Span>>, widths: &mut Vec<f64>) {
    let mut current: Vec<Span> = Vec::new();
    let mut width = 0.0;
    let commit = |current: &mut Vec<Span>, width: &mut f64, out: &mut Vec<Vec<Span>>, widths: &mut Vec<f64>, hyphen: bool| {
        if let Some(last) = current.last_mut() {
            let mut trimmed = js::trim_end(&last.text).to_string();
            if hyphen && last.text.ends_with('\u{ad}') {
                trimmed.pop();
                trimmed.push('-');
            }
            if trimmed != last.text {
                last.text = trimmed;
                let w = measure(&last.text, size, last.style);
                *width += w - last.width;
                last.width = w;
            }
        }
        out.push(std::mem::take(current));
        widths.push(*width);
        *width = 0.0;
    };
    for (si, span) in line.iter().enumerate() {
        let pieces = pieces(&span.text);
        // A piece ending in a soft hyphen fits if its '-' does, or if the rest of the word does.
        let soft = span.text.contains('\u{ad}');
        let n = if soft { pieces.len() } else { 0 };
        let mut own = vec![0f64; n];
        let mut rest = vec![0f64; n + 1];
        for q in (0..n).rev() {
            // A soft hyphen before a space is no break of its own: the space is.
            let shy = pieces[q].ends_with('\u{ad}');
            let end = js::trim_end(&pieces[q]);
            let measured = if shy { &end[..end.len() - '\u{ad}'.len_utf8()] } else { end };
            own[q] = measure(measured, size, span.style);
            // Word width beyond this span is unknown.
            rest[q] = if !shy {
                own[q]
            } else if q == n - 1 {
                if si == line.len() - 1 {
                    own[q]
                } else {
                    f64::INFINITY
                }
            } else {
                own[q] + rest[q + 1]
            };
        }
        for (p, piece) in pieces.iter().enumerate() {
            let mut part: &str = piece;
            let part_width = if soft { own[p] } else { measure(js::trim_end(part), size, span.style) };
            let fits = if !soft || !part.ends_with('\u{ad}') {
                width + part_width <= max
            } else {
                let mut hy = part[..part.len() - '\u{ad}'.len_utf8()].to_string();
                hy.push('-');
                width + measure(&hy, size, span.style) <= max || width + rest[p] <= max
            };
            if width > 0.0 && !fits {
                // Breaking at whitespace leaves a preceding soft hyphen hidden.
                commit(&mut current, &mut width, out, widths, js::trim_start(part) == part);
                part = js::trim_start(part);
            }
            match current.last_mut() {
                Some(last) if last.style == span.style => {
                    last.text.push_str(part);
                    // Remeasuring for each zero-width piece would take quadratic time.
                    if part_width > 0.0 || js::trim_end(part) != part {
                        let before = last.width;
                        last.width = measure(&last.text, size, last.style);
                        width += last.width - before;
                    }
                }
                _ => {
                    let w = measure(part, size, span.style);
                    current.push(Span { text: part.to_string(), style: span.style, width: w });
                    width += w;
                }
            }
        }
    }
    if !current.is_empty() || out.is_empty() {
        commit(&mut current, &mut width, out, widths, false);
    }
}

fn has_markup(raw: &str) -> bool {
    raw.contains(['<', '\\', '\n', '*', '_']) || ["fa:fa-", "fab:fa-", "fak:fa-", "fal:fa-", "far:fa-", "fas:fa-"].iter().any(|p| raw.contains(p))
}

pub fn layout_label(raw: Option<&str>, markdown: bool, size: f64, max_width: f64, base: u32) -> Label {
    let raw = match raw {
        None | Some("") => return Label::default(),
        Some(r) => r,
    };
    let line_height = js::round(size * LINE_HEIGHT);

    if !has_markup(raw) && !raw.contains('&') && !raw.contains('\u{fb02}') {
        let text = js::trim(raw).to_string();
        let width = measure(&text, size, base);
        if width <= max_width || !(text.contains(' ') || text.chars().any(is_cjk) || text.chars().any(is_hint)) {
            return Label { lines: vec![vec![Span { text, style: base, width }]], widths: vec![width], width, height: line_height, size, line_height };
        }
        let mut lines = Vec::new();
        let mut widths = Vec::new();
        wrap(&[Span { text, style: base, width }], size, max_width, &mut lines, &mut widths);
        let widest = widths.iter().fold(0f64, |a, &w| if w > a { w } else { a });
        let snug = if lines.len() > 1 { js::round(size * WRAPPED_LINE_HEIGHT) } else { line_height };
        return Label { height: lines.len() as f64 * snug, lines, widths, width: widest, size, line_height: snug };
    }
    layout_lines(raw, markdown, size, max_width, base, line_height)
}

/// `source.split(/<br\s*\/?>|\\n|\n/gi)`.
fn split_breaks(source: &str) -> Vec<String> {
    let c: Vec<char> = source.chars().collect();
    let n = c.len();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < n {
        if c[i] == '\n' {
            out.push(std::mem::take(&mut cur));
            i += 1;
            continue;
        }
        if c[i] == '\\' && i + 1 < n && (c[i + 1] == 'n' || c[i + 1] == 'N') {
            out.push(std::mem::take(&mut cur));
            i += 2;
            continue;
        }
        if c[i] == '<' && i + 2 < n && c[i + 1].eq_ignore_ascii_case(&'b') && c[i + 2].eq_ignore_ascii_case(&'r') {
            let mut p = i + 3;
            while p < n && js::is_space(c[p]) {
                p += 1;
            }
            if p < n && c[p] == '/' {
                p += 1;
            }
            if p < n && c[p] == '>' {
                out.push(std::mem::take(&mut cur));
                i = p + 1;
                continue;
            }
        }
        cur.push(c[i]);
        i += 1;
    }
    out.push(cur);
    out
}

fn layout_lines(raw: &str, markdown: bool, size: f64, max_width: f64, base: u32, line_height: f64) -> Label {
    let source = if markdown { js::trim(&dedent(&raw.replace("\r\n", "\n").replace('\r', "\n"))).to_string() } else { js::trim(raw).to_string() };
    let mut lines: Vec<Vec<Span>> = Vec::new();
    let mut widths: Vec<f64> = Vec::new();
    // HTML tags can span explicit breaks; Markdown emphasis stays local to each line.
    let mut open = Open { style: base, shifts: Vec::new() };
    for part in split_breaks(&source) {
        let mut line: Vec<Span> = Vec::new();
        inline(&mut line, js::trim(&part), markdown, base, &mut open);
        let w = measure_spans(&mut line, size);
        if w > max_width {
            wrap(&line, size, max_width, &mut lines, &mut widths);
        } else {
            lines.push(line);
            widths.push(w);
        }
    }
    let mut width = 0.0;
    for &w in &widths {
        if w > width {
            width = w;
        }
    }
    let snug = if lines.len() > 1 { js::round(size * WRAPPED_LINE_HEIGHT) } else { line_height };
    Label { height: lines.len() as f64 * snug, lines, widths, width, size, line_height: snug }
}
