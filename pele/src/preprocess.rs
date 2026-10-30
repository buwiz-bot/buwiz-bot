// Port of src/preprocess.ts. Works on UTF-16 code units, as upstream does, so that the edits it
// records map error positions back to the source exactly. Upstream's regexes are matched by hand.

use crate::errors::{Edit, PeleError, PeleErrorCode};
use crate::js::{self, is_space_u};
use crate::util::json::parse_json;
use crate::util::yaml::{parse_yaml, Map, Value};

pub type Config = Map;

pub struct Preprocessed {
    pub text: Vec<u16>,
    pub title: Option<String>,
    pub config: Config,
    // The text before its comment lines are removed, and the number of front matter lines before it.
    pub with_comments: Vec<u16>,
    pub line_offset: usize,
}

const NL: u16 = 10;
const CR: u16 = 13;

fn is_line_term(c: u16) -> bool {
    c == NL || c == CR || c == 0x2028 || c == 0x2029
}

fn starts_with(s: &[u16], at: usize, lit: &str) -> bool {
    let mut i = at;
    for b in lit.bytes() {
        if i >= s.len() || s[i] != b as u16 {
            return false;
        }
        i += 1;
    }
    true
}

fn contains(s: &[u16], lit: &str) -> bool {
    (0..s.len()).any(|i| starts_with(s, i, lit))
}

fn remove(text: &[u16], matches: &[(usize, usize)], steps: &mut Option<&mut Vec<Vec<Edit>>>) -> Vec<u16> {
    if let Some(st) = steps.as_mut() {
        st.push(matches.iter().map(|&(a, b)| (a, b, 0)).collect());
    }
    let mut out = Vec::with_capacity(text.len());
    let mut last = 0;
    for &(a, b) in matches {
        out.extend_from_slice(&text[last..a]);
        last = b;
    }
    out.extend_from_slice(&text[last..]);
    out
}

/// RE_COMMENT /^\s*%%(?!{)[^\n]+\n?/gm
fn comment_matches(text: &[u16]) -> Vec<(usize, usize)> {
    let n = text.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i <= n {
        if i == 0 || is_line_term(text[i - 1]) {
            let mut q = i;
            while q < n && is_space_u(text[q] as u32) {
                q += 1;
            }
            if starts_with(text, q, "%%") && text.get(q + 2) != Some(&(b'{' as u16)) {
                let mut e = q + 2;
                while e < n && text[e] != NL {
                    e += 1;
                }
                if e > q + 2 {
                    if e < n && text[e] == NL {
                        e += 1;
                    }
                    out.push((i, e));
                    i = e;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

pub fn cleanup_comments(text: &[u16], mut steps: Option<&mut Vec<Vec<Edit>>>) -> Vec<u16> {
    let text = if contains(text, "%%") { remove(text, &comment_matches(text), &mut steps) } else { text.to_vec() };
    let mut a = 0;
    while a < text.len() && is_space_u(text[a] as u32) {
        a += 1;
    }
    if let Some(st) = steps {
        st.push(vec![(0, a, 0)]);
    }
    text[a..].to_vec()
}

pub fn normalize_line_breaks(text: &[u16]) -> Vec<u16> {
    if !text.contains(&CR) {
        return text.to_vec();
    }
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i] == CR {
            out.push(NL);
            if text.get(i + 1) == Some(&NL) {
                i += 1;
            }
        } else {
            out.push(text[i]);
        }
        i += 1;
    }
    out
}

fn is_line_break(c: Option<u16>) -> bool {
    c == Some(NL) || c == Some(CR)
}

fn is_sp(c: Option<u16>) -> bool {
    c.is_some_and(|c| is_space_u(c as u32))
}

struct FrontMatter {
    indent: Vec<u16>,
    body: Vec<u16>,
    length: usize,
}

fn last_index_of(s: &[u16], needle: &[u16], from: isize) -> Option<usize> {
    if from < 0 || needle.len() > s.len() {
        return None;
    }
    let start = (from as usize).min(s.len() - needle.len());
    (0..=start).rev().find(|&i| s[i..i + needle.len()] == *needle)
}

fn index_of(s: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.len() > s.len() {
        return None;
    }
    (from..=s.len() - needle.len()).find(|&i| s[i..i + needle.len()] == *needle)
}

// Finds a `---` fenced YAML block at the start of the text the way Mermaid's front matter regex does.
fn match_front_matter(text: &[u16]) -> Option<FrontMatter> {
    let at = |i: usize| text.get(i).copied();
    let mut cursor = 0;
    while cursor < text.len() && is_sp(at(cursor)) && !is_line_break(at(cursor)) {
        cursor += 1;
    }
    let indent = text[..cursor].to_vec();
    if !starts_with(text, cursor, "---") {
        return None;
    }
    let mut opening_ends: Vec<usize> = Vec::new();
    let mut i = cursor + 3;
    while i < text.len() && is_sp(at(i)) {
        if is_line_break(at(i)) {
            opening_ends.push(i);
        }
        i += 1;
    }
    if opening_ends.is_empty() {
        return None;
    }
    let mut fence = indent.clone();
    fence.extend("---".encode_utf16());
    let closing_end = |i: usize| -> Option<usize> {
        if !is_line_break(at(i)) || !(i + 1 + fence.len() <= text.len() && text[i + 1..i + 1 + fence.len()] == fence[..]) {
            return None;
        }
        let mut last = None;
        let mut j = i + 1 + fence.len();
        while j < text.len() && is_sp(at(j)) {
            if is_line_break(at(j)) {
                last = Some(j);
            }
            j += 1;
        }
        last
    };
    let mut needle = vec![NL];
    needle.extend_from_slice(&fence);
    let mut last_fence: Option<usize> = None;
    let mut i = last_index_of(text, &needle, text.len() as isize);
    while let Some(k) = i {
        if k <= opening_ends[0] {
            break;
        }
        if closing_end(k).is_some() {
            last_fence = Some(k);
            break;
        }
        i = if k == 0 { None } else { last_index_of(text, &needle, k as isize - 1) };
    }
    let last_fence = last_fence?;
    let mut opening_end = opening_ends[0];
    for &c in &opening_ends {
        if c < last_fence {
            opening_end = c;
        }
    }
    let body_start = opening_end + 1;
    let first_at = if body_start > 0 { body_start - 1 } else { 0 };
    let mut i = index_of(text, &needle, first_at);
    while let Some(k) = i {
        if k > last_fence {
            break;
        }
        if k >= body_start {
            if let Some(end) = closing_end(k) {
                return Some(FrontMatter { indent, body: text[body_start..k].to_vec(), length: end + 1 });
            }
        }
        i = index_of(text, &needle, k + 1);
    }
    None
}

const UNSAFE_KEYS: [&str; 3] = ["__proto__", "constructor", "prototype"];

fn merge(target: &mut Map, source: &Map) {
    for (key, value) in &source.0 {
        if UNSAFE_KEYS.contains(&key.as_str()) {
            continue;
        }
        match (value, target.get_mut(key)) {
            (Value::Map(v), Some(Value::Map(current))) => merge(current, v),
            (Value::Map(v), _) => {
                let mut fresh = Map::default();
                merge(&mut fresh, v);
                target.set(key, Value::Map(fresh));
            }
            (v, _) => target.set(key, v.clone()),
        }
    }
}

fn is_w(c: u16) -> bool {
    crate::util::chars::is_word(c as u32)
}

/// One match of RE_DIRECTIVE at `i`: (end, type, value group 3 or 4).
fn directive_at(s: &[u16], i: usize) -> Option<(usize, Vec<u16>, Option<Vec<u16>>)> {
    let n = s.len();
    if !starts_with(s, i, "%%{") {
        return None;
    }
    let ws = |mut q: usize| {
        while q < n && is_space_u(s[q] as u32) {
            q += 1;
        }
        q
    };
    let mut q = ws(i + 3);
    let ts = q;
    while q < n && is_w(s[q]) {
        q += 1;
    }
    if q == ts {
        return None;
    }
    let type_ = s[ts..q].to_vec();
    let after = ws(q);
    q = if after < n && s[after] == b':' as u16 { after + 1 } else { q };
    q = ws(q);
    // (?:(\w+)|((?:(?!}%{2}).|\r?\n)*))?
    let value;
    let vs = q;
    while q < n && is_w(s[q]) {
        q += 1;
    }
    if q > vs {
        value = Some(s[vs..q].to_vec());
    } else {
        loop {
            if starts_with(s, q, "}%%") {
                break;
            }
            if q < n && !is_line_term(s[q]) {
                q += 1;
            } else if q < n && s[q] == NL {
                q += 1;
            } else if q + 1 < n && s[q] == CR && s[q + 1] == NL {
                q += 2;
            } else {
                break;
            }
        }
        value = Some(s[vs..q].to_vec());
    }
    q = ws(q);
    if starts_with(s, q, "}%%") {
        q += 3;
    }
    Some((q, type_, value))
}

fn directive_matches(s: &[u16]) -> Vec<(usize, usize, Vec<u16>, Option<Vec<u16>>, bool)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if let Some((end, ty, value)) = directive_at(s, i) {
            // Whether the value came from the `\w+` alternative (group 3) rather than group 4.
            let word = value.as_ref().is_some_and(|v| !v.is_empty() && v.iter().all(|&c| is_w(c)));
            out.push((i, end, ty, value, word));
            i = end.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

fn directives(text: &[u16], config: &mut Config, steps: &mut Option<&mut Vec<Vec<Edit>>>) -> Vec<u16> {
    if !contains(text, "%%{") {
        return text.to_vec();
    }
    let trimmed = js::u16s(js::trim(&js::from16(text)));
    let normalized: Vec<u16> = trimmed.iter().map(|&c| if c == b'\'' as u16 { b'"' as u16 } else { c }).collect();
    for (_, _, ty, value, word) in directive_matches(&normalized) {
        let ty = js::from16(&ty);
        if ty == "wrap" {
            config.set("wrap", Value::Bool(true));
        } else if ty == "init" || ty == "initialize" {
            // match[4] only: a single word is group 3.
            if let Some(v) = value.filter(|v| !word && !v.is_empty()) {
                if let Ok(Value::Map(args)) = parse_json(js::trim(&js::from16(&v))) {
                    merge(config, &args);
                }
            }
        }
    }
    let matches: Vec<(usize, usize)> = directive_matches(text).iter().map(|m| (m.0, m.1)).collect();
    remove(text, &matches, steps)
}

// `steps` maps normalized source to `with_comments`.
pub fn preprocess(source: &[u16], mut steps: Option<&mut Vec<Vec<Edit>>>) -> Result<Preprocessed, PeleError> {
    let mut text = normalize_line_breaks(source);
    // A tag needs a closing bracket, so nothing after the last one can match.
    let last_close = if text.contains(&(b'<' as u16)) { text.iter().rposition(|&c| c == b'>' as u16) } else { None };
    if let Some(lc) = last_close.filter(|&l| l > 0) {
        html_tag_quotes(&mut text[..lc + 1]);
    }

    let mut config = Config::default();
    let mut title: Option<String> = None;
    let mut line_offset = 0;

    let front = if contains(&text, "---") { match_front_matter(&text) } else { None };
    if let Some(front) = front {
        let body: Vec<u16> = if !front.indent.is_empty() {
            let mut out: Vec<u16> = Vec::new();
            for (k, line) in front.body.split(|&c| c == NL).enumerate() {
                if k > 0 {
                    out.push(NL);
                }
                if line.starts_with(&front.indent) {
                    out.extend_from_slice(&line[front.indent.len()..]);
                } else {
                    out.extend_from_slice(line);
                }
            }
            out
        } else {
            front.body.clone()
        };
        let parsed = parse_yaml(&js::from16(&body)).map_err(|m| PeleError::new(format!("Front matter is not valid YAML. {}", m), PeleErrorCode::Syntax))?;
        if let Value::Map(parsed) = parsed {
            if let Some(t) = parsed.get("title").filter(|t| t.truthy()) {
                title = Some(t.js_string());
            }
            if let Some(Value::Map(c)) = parsed.get("config") {
                merge(&mut config, c);
            }
            if let Some(d) = parsed.get("displayMode").filter(|d| d.truthy()) {
                let mode = Value::Str(d.js_string());
                match config.get_mut("gantt") {
                    Some(Value::Map(g)) => g.set("displayMode", mode),
                    _ => {
                        let mut g = Map::default();
                        g.set("displayMode", mode);
                        config.set("gantt", Value::Map(g));
                    }
                }
            }
        }
        line_offset = text[..front.length.min(text.len())].iter().filter(|&&c| c == NL).count();
        text = text[front.length..].to_vec();
        if let Some(st) = steps.as_mut() {
            st.push(vec![(0, front.length, 0)]);
        }
    }

    let text = directives(&text, &mut config, &mut steps);
    Ok(Preprocessed { text: cleanup_comments(&text, None), title, config, with_comments: text, line_offset })
}

/// RE_HTML_TAG with RE_ATTR: `<tag a="b">` becomes `<tag a='b'>`. The length does not change.
fn html_tag_quotes(text: &mut [u16]) {
    let n = text.len();
    let mut i = 0;
    while i < n {
        if text[i] == b'<' as u16 && i + 1 < n && is_w(text[i + 1]) {
            let mut q = i + 1;
            while q < n && is_w(text[q]) {
                q += 1;
            }
            let attrs_start = q;
            while q < n && text[q] != b'>' as u16 {
                q += 1;
            }
            if q < n {
                // attrs.replace(/="([^"]*)"/g, "='$1'")
                let mut k = attrs_start;
                while k + 1 < q {
                    if text[k] == b'=' as u16 && text[k + 1] == b'"' as u16 {
                        if let Some(close) = (k + 2..q).find(|&j| text[j] == b'"' as u16) {
                            text[k + 1] = b'\'' as u16;
                            text[close] = b'\'' as u16;
                            k = close + 1;
                            continue;
                        }
                    }
                    k += 1;
                }
                i = q + 1;
                continue;
            }
        }
        i += 1;
    }
}

fn record(steps: &mut Option<&mut Vec<Vec<Edit>>>) -> Option<usize> {
    steps.as_mut().map(|st| {
        st.push(Vec::new());
        st.len() - 1
    })
}

// Does what Mermaid's `text.replace(/keyword.*:\S*#.*;/g, drop the last character)` does: on a
// line where the keyword is followed by a colon, a color-like `#`, and later a semicolon, the
// last semicolon goes.
fn drop_color_semicolons(text: Vec<u16>, keyword: &str, steps: &mut Option<&mut Vec<Vec<Edit>>>) -> Vec<u16> {
    let slot = record(steps);
    let kw: Vec<u16> = keyword.encode_utf16().collect();
    let Some(mut from) = index_of(&text, &kw, 0) else { return text };
    let mut out: Vec<u16> = Vec::new();
    let mut last = 0;
    let mut edits: Vec<Edit> = Vec::new();
    loop {
        let line_end = (from..text.len()).find(|&i| is_line_term(text[i])).unwrap_or(text.len());
        let mut after_colon = false;
        let mut hash: Option<usize> = None;
        for j in from + kw.len()..line_end {
            let c = text[j];
            if c == 58 {
                after_colon = true;
            } else if c == 35 {
                if after_colon {
                    hash = Some(j);
                    break;
                }
            } else if is_space_u(c as u32) {
                after_colon = false;
            }
        }
        if let Some(hash) = hash {
            if line_end > 0 {
                if let Some(semi) = (0..line_end).rev().find(|&i| text[i] == 59) {
                    if semi > hash {
                        edits.push((semi, semi + 1, 0));
                        out.extend_from_slice(&text[last..semi]);
                        last = semi + 1;
                    }
                }
            }
        }
        if line_end >= text.len() {
            break;
        }
        match index_of(&text, &kw, line_end + 1) {
            Some(f) => from = f,
            None => break,
        }
    }
    if let (Some(st), Some(slot)) = (steps.as_mut(), slot) {
        st[slot] = edits;
    }
    if last == 0 {
        return text;
    }
    out.extend_from_slice(&text[last..]);
    out
}

// Mermaid hides `#name;` entity codes behind placeholder characters so `#` and `;` survive parsing.
pub fn encode_entities(text: &[u16], mut steps: Option<&mut Vec<Vec<Edit>>>) -> Vec<u16> {
    if !text.contains(&35) {
        return text.to_vec();
    }
    let text = drop_color_semicolons(text.to_vec(), "style", &mut steps);
    let text = drop_color_semicolons(text, "classDef", &mut steps);
    let slot = record(&mut steps);
    let mut edits: Vec<Edit> = Vec::new();
    let mut out: Vec<u16> = Vec::with_capacity(text.len());
    let mut i = 0;
    // The offset into the output, for the edits.
    while i < text.len() {
        if text[i] == 35 {
            let mut q = i + 1;
            while q < text.len() && is_w(text[q]) {
                q += 1;
            }
            if q > i + 1 && q < text.len() && text[q] == 59 {
                let inner = &text[i + 1..q];
                let numeric = inner.iter().all(|&c| (48..=57).contains(&c));
                let mut code: Vec<u16> = if numeric { "\u{fb02}\u{b0}\u{b0}".encode_utf16().collect() } else { "\u{fb02}\u{b0}".encode_utf16().collect() };
                code.extend_from_slice(inner);
                code.extend("\u{b6}\u{df}".encode_utf16());
                edits.push((i, q + 1, code.len()));
                out.extend_from_slice(&code);
                i = q + 1;
                continue;
            }
        }
        out.push(text[i]);
        i += 1;
    }
    if let (Some(st), Some(slot)) = (steps.as_mut(), slot) {
        st[slot] = edits;
    }
    out
}
