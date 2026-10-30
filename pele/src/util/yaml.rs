// Port of src/util/yaml.ts: a small YAML reader covering what Mermaid feeds to js-yaml with the
// JSON schema: block and flow collections, quoted and plain scalars, and block scalars.
// Strings are read as chars; upstream reads UTF-16 units, which only differs in how far an
// index moves past a character outside the BMP, and every test here is on ASCII.

use crate::js;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Seq(Vec<Value>),
    Map(Map),
}

/// An object, in insertion order. (JS orders integer-like keys first; nothing the port reads
/// depends on key order.)
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Map(pub Vec<(String, Value)>);

impl Map {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.0.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn set(&mut self, key: &str, value: Value) {
        match self.get_mut(key) {
            Some(slot) => *slot = value,
            None => self.0.push((key.to_string(), value)),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Value {
    /// JS truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Num(n) => *n != 0.0 && !n.is_nan(),
            Value::Str(s) => !s.is_empty(),
            _ => true,
        }
    }
    /// `String(value)`.
    pub fn js_string(&self) -> String {
        match self {
            Value::Null => "null".into(),
            Value::Bool(b) => b.to_string(),
            Value::Num(n) => js::num_str(*n),
            Value::Str(s) => s.clone(),
            Value::Seq(items) => items
                .iter()
                .map(|v| if matches!(v, Value::Null) { String::new() } else { v.js_string() })
                .collect::<Vec<_>>()
                .join(","),
            Value::Map(_) => "[object Object]".into(),
        }
    }
    /// `Number(value)`.
    pub fn js_number(&self) -> f64 {
        match self {
            Value::Null => 0.0,
            Value::Bool(b) => *b as u8 as f64,
            Value::Num(n) => *n,
            Value::Str(s) => js::to_number(s),
            Value::Seq(items) => match items.len() {
                0 => 0.0,
                1 => Value::Str(items[0].js_string()).js_number(),
                _ => f64::NAN,
            },
            Value::Map(_) => f64::NAN,
        }
    }
    pub fn as_map(&self) -> Option<&Map> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct YamlError {
    // Thrown when a quoted scalar or flow collection runs past the text read so far.
    incomplete: bool,
    pub message: String,
}

fn err(message: &str) -> YamlError {
    YamlError { incomplete: false, message: message.into() }
}

fn incomplete(message: &str) -> YamlError {
    YamlError { incomplete: true, message: message.into() }
}

type R<T> = Result<T, YamlError>;

const MAX_DEPTH: usize = 64;

struct Reader {
    lines: Vec<Vec<char>>,
    i: usize,
}

fn trim_chars(s: &[char]) -> &[char] {
    let mut a = 0;
    let mut b = s.len();
    while a < b && js::is_space(s[a]) {
        a += 1;
    }
    while b > a && js::is_space(s[b - 1]) {
        b -= 1;
    }
    &s[a..b]
}

fn trim_start_len(s: &[char]) -> usize {
    let mut a = 0;
    while a < s.len() && js::is_space(s[a]) {
        a += 1;
    }
    s.len() - a
}

fn s(chars: &[char]) -> String {
    chars.iter().collect()
}

impl Reader {
    fn new(src: &str) -> Reader {
        let src = if src.contains('\r') { src.replace("\r\n", "\n").replace('\r', "\n") } else { src.to_string() };
        Reader { lines: src.split('\n').map(|l| l.chars().collect()).collect(), i: 0 }
    }

    fn skip_blank(&mut self) {
        while self.i < self.lines.len() {
            let t = trim_chars(&self.lines[self.i]);
            if !t.is_empty() && t[0] != '#' && t != ['-', '-', '-'] {
                break;
            }
            self.i += 1;
        }
    }

    fn indent(&self) -> R<usize> {
        let line = &self.lines[self.i];
        let mut k = 0;
        while k < line.len() && line[k] == ' ' {
            k += 1;
        }
        if line.get(k) == Some(&'\t') {
            return Err(err("YAML: tabs are not allowed for indentation"));
        }
        Ok(k)
    }
}

fn is_blank(c: Option<char>) -> bool {
    matches!(c, Some(' ' | '\t' | '\n' | '\r'))
}

fn is_key_start(c: char) -> bool {
    js::is_space(c) || "#:,[]{}&*!|>'\"%@`".contains(c)
}

// Splits `key: rest` at the start of a line. Returns the raw key and the offset where the value starts.
fn split_key(line: &[char]) -> Option<(Vec<char>, usize)> {
    let first = line.first().copied();
    let at = |i: usize| line.get(i).copied();
    let end;
    let mut colon;
    if first == Some('"') || first == Some('\'') {
        let first = first.unwrap();
        let mut i = 1;
        loop {
            if i >= line.len() {
                return None;
            }
            let c = line[i];
            if first == '"' && c == '\\' {
                i += 2;
            } else if c == first {
                if first == '"' || at(i + 1) != Some('\'') {
                    break;
                }
                i += 2;
            } else {
                i += 1;
            }
        }
        end = i + 1;
        colon = end;
        while is_blank(at(colon)) {
            colon += 1;
        }
        if at(colon) != Some(':') {
            return None;
        }
    } else {
        let first = first?;
        if is_key_start(first) {
            return None;
        }
        if first == '-' && (line.len() < 2 || is_blank(at(1)) || at(1) == Some(':') || at(1) == Some('#')) {
            return None;
        }
        colon = 1;
        while colon < line.len() && line[colon] != ':' && line[colon] != '#' {
            colon += 1;
        }
        if at(colon) != Some(':') {
            return None;
        }
        let mut e = colon;
        while e > 0 && is_blank(at(e - 1)) {
            e -= 1;
        }
        end = e;
    }
    let mut length = colon + 1;
    if length < line.len() && !is_blank(at(length)) {
        return None;
    }
    while is_blank(at(length)) {
        length += 1;
    }
    Some((line[..end].to_vec(), length))
}

fn all_match(s: &str, f: impl Fn(char) -> bool) -> bool {
    !s.is_empty() && s.chars().all(f)
}

fn sign_split(s: &str) -> &str {
    s.strip_prefix(['-', '+']).unwrap_or(s)
}

fn is_int(s: &str) -> bool {
    let b = sign_split(s);
    b == "0" || (all_match(b, |c| c.is_ascii_digit()) && !b.starts_with('0'))
}

fn is_float(s: &str) -> bool {
    // ^[-+]?(?:[0-9][0-9]*(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][-+]?[0-9]+)?$
    let b: Vec<char> = sign_split(s).chars().collect();
    let mut i = 0;
    if i < b.len() && b[i].is_ascii_digit() {
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i < b.len() && b[i] == '.' {
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
    } else if i < b.len() && b[i] == '.' {
        i += 1;
        let st = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == st {
            return false;
        }
    } else {
        return false;
    }
    if i < b.len() && (b[i] == 'e' || b[i] == 'E') {
        i += 1;
        if i < b.len() && (b[i] == '-' || b[i] == '+') {
            i += 1;
        }
        let st = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == st {
            return false;
        }
    }
    i == b.len()
}

fn is_radix(s: &str, prefix: &str, digit: impl Fn(char) -> bool) -> bool {
    sign_split(s).strip_prefix(prefix).is_some_and(|r| all_match(r, digit))
}

fn scalar(raw: &str) -> Value {
    let s = js::trim(raw);
    match s {
        "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        _ => {}
    }
    let c = s.chars().next().unwrap();
    if c != '-' && c != '+' && c != '.' && !c.is_ascii_digit() {
        return Value::Str(s.into());
    }
    if is_int(s) || is_float(s) {
        return Value::Num(js::to_number(s));
    }
    if is_radix(s, "0x", |c| c.is_ascii_hexdigit()) || is_radix(s, "0o", |c| ('0'..='7').contains(&c)) || is_radix(s, "0b", |c| c == '0' || c == '1') {
        let neg = s.starts_with('-');
        let v = js::to_number(sign_split(s));
        return Value::Num(if neg { -v } else { v });
    }
    let b = sign_split(s);
    if b == ".inf" || b == ".Inf" || b == ".INF" {
        return Value::Num(if s.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY });
    }
    if s == ".nan" || s == ".NaN" || s == ".NAN" {
        return Value::Num(f64::NAN);
    }
    Value::Str(s.into())
}

fn escape(c: char) -> Option<&'static str> {
    Some(match c {
        '0' => "\0",
        'a' => "\x07",
        'b' => "\x08",
        't' => "\t",
        'n' => "\n",
        'v' => "\x0b",
        'f' => "\x0c",
        'r' => "\r",
        'e' => "\x1b",
        ' ' => " ",
        '"' => "\"",
        '/' => "/",
        '\\' => "\\",
        'N' => "\u{85}",
        '_' => "\u{a0}",
        'L' => "\u{2028}",
        'P' => "\u{2029}",
        _ => return None,
    })
}

// Folds the line breaks of a scalar: blanks around a break go, one break becomes a space, and
// each further break stays.
fn fold(text: &str) -> String {
    if !text.contains('\n') {
        return text.to_string();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let last = lines.len() - 1;
    let mut parts = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        let mut l: &str = line;
        if i > 0 {
            l = l.trim_start_matches([' ', '\t']);
        }
        if i < last {
            l = l.trim_end_matches([' ', '\t']);
        }
        parts.push(l);
    }
    let joined = parts.join("\n");
    let mut out = String::with_capacity(joined.len());
    let mut run = 0;
    for c in joined.chars() {
        if c == '\n' {
            run += 1;
            continue;
        }
        if run > 0 {
            if run == 1 {
                out.push(' ');
            } else {
                out.push_str(&"\n".repeat(run - 1));
            }
            run = 0;
        }
        out.push(c);
    }
    if run > 0 {
        if run == 1 {
            out.push(' ');
        } else {
            out.push_str(&"\n".repeat(run - 1));
        }
    }
    out
}

fn unquote_double(body: &str) -> R<String> {
    if !body.contains('\\') {
        return Ok(fold(body));
    }
    // body.replace(/\\\n[ \t]*/g, '')
    let mut cleaned = String::new();
    let chars: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && chars.get(i + 1) == Some(&'\n') {
            i += 2;
            while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
                i += 1;
            }
            continue;
        }
        cleaned.push(chars[i]);
        i += 1;
    }
    let s: Vec<char> = fold(&cleaned).chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < s.len() {
        let ch = s[i];
        if ch != '\\' {
            out.push(ch);
            i += 1;
            continue;
        }
        i += 1;
        let e = s.get(i).copied();
        match e {
            Some(e @ ('x' | 'u' | 'U')) => {
                let len = if e == 'x' { 2 } else if e == 'u' { 4 } else { 8 };
                let hex: String = s[(i + 1).min(s.len())..(i + 1 + len).min(s.len())].iter().collect();
                let code = js::parse_int(&hex, 16);
                match char::from_u32(code as u32).filter(|_| code.is_finite() && code >= 0.0) {
                    Some(c) => out.push(c),
                    None => return Err(err("Invalid code point NaN")),
                }
                i += len;
            }
            Some(e) if escape(e).is_some() => out.push_str(escape(e).unwrap()),
            _ => return Err(err("YAML: unknown escape sequence")),
        }
        i += 1;
    }
    Ok(out)
}

fn skip(s: &[char], mut pos: usize, commas: bool) -> usize {
    while pos < s.len() {
        let c = s[pos];
        if c == ' ' || c == '\n' || c == '\t' || c == '\r' || (commas && c == ',') {
            pos += 1;
        } else if c == '#' && (pos == 0 || js::is_space(s[pos - 1])) {
            pos = s[pos..].iter().position(|&c| c == '\n').map_or(s.len(), |p| p + pos);
        } else {
            break;
        }
    }
    pos
}

// Entries of a flow collection are separated by exactly one comma, as js-yaml requires.
fn entry_start(ch: char, separated: bool) -> R<()> {
    if !separated {
        return Err(err("YAML: missed comma between flow collection entries"));
    }
    if ch == ',' {
        return Err(err("YAML: expected the node content, but found ','"));
    }
    Ok(())
}

// Parses a flow value starting at s[pos]; returns the value and the index after it.
fn flow(s: &[char], pos: usize, stops: &str, depth: usize) -> R<(Value, usize)> {
    if depth > MAX_DEPTH {
        return Err(err("YAML: nesting is too deep"));
    }
    let mut pos = skip(s, pos, false);
    let ch = s.get(pos).copied();
    if ch == Some('{') {
        let mut out = Map::default();
        pos += 1;
        let mut separated = true;
        loop {
            pos = skip(s, pos, false);
            if pos >= s.len() {
                return Err(incomplete("YAML: unexpected end of the stream within a flow collection"));
            }
            if s[pos] == '}' {
                return Ok((Value::Map(out), pos + 1));
            }
            entry_start(s[pos], separated)?;
            let (key, p) = flow(s, pos, ":,}", depth + 1)?;
            pos = skip(s, p, false);
            let mut value = Value::Null;
            if s.get(pos) == Some(&':') {
                let (v, p) = flow(s, pos + 1, ",}", depth + 1)?;
                value = v;
                pos = p;
            }
            out.set(&key.js_string(), value);
            pos = skip(s, pos, false);
            separated = s.get(pos) == Some(&',');
            if separated {
                pos += 1;
            }
        }
    }
    if ch == Some('[') {
        let mut out = Vec::new();
        pos += 1;
        let mut separated = true;
        loop {
            pos = skip(s, pos, false);
            if pos >= s.len() {
                return Err(incomplete("YAML: unexpected end of the stream within a flow collection"));
            }
            if s[pos] == ']' {
                return Ok((Value::Seq(out), pos + 1));
            }
            entry_start(s[pos], separated)?;
            let (value, p) = flow(s, pos, ",]", depth + 1)?;
            out.push(value);
            pos = skip(s, p, false);
            separated = s.get(pos) == Some(&',');
            if separated {
                pos += 1;
            }
        }
    }
    if ch == Some('"') {
        let mut end = pos + 1;
        while end < s.len() && s[end] != '"' {
            end += if s[end] == '\\' { 2 } else { 1 };
        }
        if end >= s.len() {
            return Err(incomplete("YAML: unexpected end of the stream within a double quoted scalar"));
        }
        return Ok((Value::Str(unquote_double(&self::s(&s[pos + 1..end]))?), end + 1));
    }
    if ch == Some('\'') {
        let mut end = pos + 1;
        while end < s.len() {
            if s[end] == '\'' {
                if s.get(end + 1) != Some(&'\'') {
                    break;
                }
                end += 1;
            }
            end += 1;
        }
        if end >= s.len() {
            return Err(incomplete("YAML: unexpected end of the stream within a single quoted scalar"));
        }
        return Ok((Value::Str(fold(&self::s(&s[pos + 1..end])).replace("''", "'")), end + 1));
    }
    let mut end = pos;
    while end < s.len() {
        let c = s[end];
        if stops.contains(c) && (c != ':' || end + 1 >= s.len() || js::is_space(s[end + 1]) || ",[]{}".contains(s[end + 1])) {
            break;
        }
        if c == '#' && end > pos && js::is_space(s[end - 1]) {
            let nl = s[end..].iter().position(|&c| c == '\n').map_or(s.len(), |p| p + end);
            return Ok((scalar(&fold(js::trim(&self::s(&s[pos..end])))), nl));
        }
        end += 1;
    }
    Ok((scalar(&fold(js::trim(&self::s(&s[pos.min(end)..end])))), end))
}

fn strip_comment(s: &[char]) -> &[char] {
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if let Some(q) = quote {
            if c == '\\' && q == '"' {
                i += 1;
            } else if c == q {
                quote = None;
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        } else if c == '#' && (i == 0 || s[i - 1] == ' ' || s[i - 1] == '\t') {
            return &s[..i];
        }
        i += 1;
    }
    s
}

fn block_scalar(r: &mut Reader, header: &[char], parent_indent: isize) -> String {
    let folded = header[0] == '>';
    let chomp = if header.contains(&'-') { '-' } else if header.contains(&'+') { '+' } else { ' ' };
    let explicit = header.iter().find(|c| ('1'..='9').contains(*c));
    let mut indent: isize = match explicit {
        Some(c) => parent_indent + c.to_digit(10).unwrap() as isize,
        None => -1,
    };
    let mut out: Vec<Vec<char>> = Vec::new();
    while r.i < r.lines.len() {
        let line = &r.lines[r.i];
        if trim_chars(line).is_empty() {
            out.push(Vec::new());
            r.i += 1;
            continue;
        }
        let k = line.len() - trim_start_len(line);
        if indent == -1 {
            if k as isize <= parent_indent {
                break;
            }
            indent = k as isize;
        }
        if (k as isize) < indent {
            break;
        }
        out.push(line[indent as usize..].to_vec());
        r.i += 1;
    }
    let mut trailing = 0;
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
        trailing += 1;
    }
    let text = if folded {
        let mut text = String::new();
        for i in 0..out.len() {
            let cur = &out[i];
            if i > 0 {
                let prev = &out[i - 1];
                if cur.is_empty() {
                    text.push('\n');
                } else if !prev.is_empty() {
                    let lead = |l: &Vec<char>| l.first().is_some_and(|c| *c == ' ' || *c == '\t');
                    text.push(if lead(prev) || lead(cur) { '\n' } else { ' ' });
                }
            }
            text.push_str(&s(cur));
        }
        text
    } else {
        out.iter().map(|l| s(l)).collect::<Vec<_>>().join("\n")
    };
    if out.is_empty() {
        return if chomp == '+' { "\n".repeat(trailing) } else { String::new() };
    }
    match chomp {
        '-' => text,
        '+' => text + &"\n".repeat(trailing + 1),
        _ => text + "\n",
    }
}

fn is_block_header(rest: &[char]) -> bool {
    // /^[|>][-+0-9]*\s*(?:#.*)?$/
    let mut i = 1;
    while i < rest.len() && (rest[i] == '-' || rest[i] == '+' || rest[i].is_ascii_digit()) {
        i += 1;
    }
    while i < rest.len() && js::is_space(rest[i]) {
        i += 1;
    }
    if i == rest.len() {
        return true;
    }
    if rest[i] != '#' {
        return false;
    }
    !rest[i..].iter().any(|&c| c == '\n' || c == '\r' || c == '\u{2028}' || c == '\u{2029}')
}

fn inline_value(r: &mut Reader, rest: &[char], indent: isize) -> R<Value> {
    let first = rest.first().copied();
    if (first == Some('|') || first == Some('>')) && is_block_header(rest) {
        return Ok(Value::Str(block_scalar(r, rest, indent)));
    }
    if matches!(first, Some('{' | '[' | '"' | '\'')) {
        // The value may continue on later lines. Read them in growing batches until it closes.
        let start = r.i;
        let mut text: Vec<char> = rest.to_vec();
        let mut taken = 0;
        let mut batch = 1;
        loop {
            match flow(&text, 0, "", 0) {
                Ok((value, end)) => {
                    let used = text[..end.min(text.len())].iter().filter(|&&c| c == '\n').count();
                    let line_end = text[end.min(text.len())..].iter().position(|&c| c == '\n').map_or(text.len(), |p| p + end);
                    if !trim_chars(strip_comment(&text[end.min(text.len())..line_end])).is_empty() {
                        return Err(err("YAML: unexpected content after a value"));
                    }
                    r.i = start + used;
                    return Ok(value);
                }
                Err(e) if e.incomplete => {
                    if start + taken >= r.lines.len() {
                        return Err(err(&e.message));
                    }
                    let more: Vec<Vec<char>> = r.lines[start + taken..(start + taken + batch).min(r.lines.len())].to_vec();
                    for line in &more {
                        text.push('\n');
                        text.extend_from_slice(line);
                    }
                    if more.is_empty() {
                        text.push('\n');
                    }
                    taken += more.len();
                    batch *= 2;
                }
                Err(e) => return Err(e),
            }
        }
    }
    let mut text = s(trim_chars(strip_comment(rest)));
    while r.i < r.lines.len() {
        let line = &r.lines[r.i];
        if trim_chars(line).is_empty() {
            let mut j = r.i;
            while j < r.lines.len() && trim_chars(&r.lines[j]).is_empty() {
                j += 1;
            }
            if j >= r.lines.len() || (r.lines[j].len() - trim_start_len(&r.lines[j])) as isize <= indent {
                break;
            }
            text.push_str(&"\n".repeat(j - r.i));
            r.i = j;
            continue;
        }
        if (line.len() - trim_start_len(line)) as isize <= indent {
            break;
        }
        let more = s(trim_chars(strip_comment(line)));
        if !text.ends_with('\n') {
            text.push(' ');
        }
        text.push_str(&more);
        r.i += 1;
    }
    Ok(scalar(&text))
}

fn is_seq_line(l: &[char]) -> bool {
    l == ['-'] || l.starts_with(&['-', ' '])
}

fn block(r: &mut Reader, min_indent: usize, depth: usize) -> R<Value> {
    if depth > MAX_DEPTH {
        return Err(err("YAML: nesting is too deep"));
    }
    r.skip_blank();
    if r.i >= r.lines.len() {
        return Ok(Value::Null);
    }
    let indent = r.indent()?;
    if indent < min_indent {
        return Ok(Value::Null);
    }
    let line: Vec<char> = r.lines[r.i][indent..].to_vec();

    if is_seq_line(&line) {
        let mut out = Vec::new();
        while r.i < r.lines.len() {
            r.skip_blank();
            if r.i >= r.lines.len() || r.indent()? != indent {
                break;
            }
            let cur: Vec<char> = r.lines[r.i][indent..].to_vec();
            if !is_seq_line(&cur) {
                break;
            }
            let rest = &cur[1..];
            let pad = rest.len() - trim_start_len(rest) + 1;
            let body = trim_chars(strip_comment(rest));
            if body.is_empty() {
                r.i += 1;
                out.push(block(r, indent + 1, depth + 1)?);
            } else if split_key(body).is_some() {
                let mut l: Vec<char> = vec![' '; indent + pad];
                l.extend_from_slice(&rest[rest.len() - trim_start_len(rest)..]);
                r.lines[r.i] = l;
                out.push(block(r, indent + pad, depth + 1)?);
            } else {
                r.i += 1;
                out.push(inline_value(r, trim_chars(rest), indent as isize)?);
            }
        }
        return Ok(Value::Seq(out));
    }

    if split_key(&line).is_none() {
        r.i += 1;
        return inline_value(r, trim_chars(&line), indent as isize - 1);
    }

    let mut out = Map::default();
    while r.i < r.lines.len() {
        r.skip_blank();
        if r.i >= r.lines.len() {
            break;
        }
        let k = r.indent()?;
        if k < indent {
            break;
        }
        if k > indent {
            return Err(err("YAML: bad indentation of a mapping entry"));
        }
        let cur: Vec<char> = r.lines[r.i][indent..].to_vec();
        let Some((raw_key, length)) = split_key(&cur) else {
            return Err(err("YAML: can not read a block mapping entry"));
        };
        let key = flow(&raw_key, 0, "", 0)?.0.js_string();
        let rest: Vec<char> = cur[length.min(cur.len())..].to_vec();
        r.i += 1;
        if trim_chars(strip_comment(&rest)).is_empty() {
            r.skip_blank();
            if r.i < r.lines.len() {
                let next = r.indent()?;
                let next_line: Vec<char> = r.lines[r.i][next..].to_vec();
                let seq = is_seq_line(&next_line);
                let value = if next > indent || (seq && next == indent) { block(r, if seq { indent } else { indent + 1 }, depth + 1)? } else { Value::Null };
                out.set(&key, value);
            } else {
                out.set(&key, Value::Null);
            }
        } else {
            let v = inline_value(r, trim_chars(&rest), indent as isize)?;
            out.set(&key, v);
        }
    }
    Ok(Value::Map(out))
}

pub fn parse_yaml(src: &str) -> Result<Value, String> {
    let trimmed: Vec<char> = js::trim(src).chars().collect();
    if trimmed.first() == Some(&'{') || trimmed.first() == Some(&'[') {
        match flow(&trimmed, 0, "", 0) {
            Ok((value, end)) => {
                if skip(&trimmed, end, false) == trimmed.len() {
                    return Ok(value);
                }
            }
            Err(e) => return Err(e.message),
        }
    }
    let mut r = Reader::new(src);
    let value = block(&mut r, 0, 0).map_err(|e| e.message)?;
    r.skip_blank();
    if r.i < r.lines.len() && trim_chars(&r.lines[r.i]) != ['.', '.', '.'] {
        return Err("YAML: end of the stream or a document separator is expected".into());
    }
    Ok(value)
}
