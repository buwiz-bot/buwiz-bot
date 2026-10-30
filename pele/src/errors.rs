// Port of src/errors.ts. Offsets, lines and columns count UTF-16 code units, as upstream's do.

use std::cell::RefCell;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PeleErrorCode {
    UnsupportedDiagram,
    Syntax,
    Semantic,
    Limit,
    /// Not upstream: a type that upstream renders but this port does not yet. The host draws it
    /// some other way.
    NotYetPorted,
}

impl PeleErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            PeleErrorCode::UnsupportedDiagram => "unsupported-diagram",
            PeleErrorCode::Syntax => "syntax",
            PeleErrorCode::Semantic => "semantic",
            PeleErrorCode::Limit => "limit",
            PeleErrorCode::NotYetPorted => "not-yet-ported",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PeleError {
    pub message: String,
    pub code: PeleErrorCode,
    pub type_: Option<String>,
    pub mermaid_type: Option<String>,
    pub line: usize,
    pub column: usize,
    pub snippet: String,
}

impl PeleError {
    pub fn new(message: impl Into<String>, code: PeleErrorCode) -> Self {
        PeleError { message: message.into(), code, type_: None, mermaid_type: None, line: 0, column: 0, snippet: String::new() }
    }

    pub fn typed(message: impl Into<String>, code: PeleErrorCode, type_: &str) -> Self {
        let mut e = PeleError::new(message, code);
        e.type_ = Some(type_.to_string());
        e
    }
}

impl fmt::Display for PeleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PeleError {}

/// Input range [from, to) replaced by `length` output characters.
pub type Edit = (usize, usize, usize);

pub fn step_input(edits: &[Edit], at: usize) -> usize {
    let mut shift: isize = 0;
    let at = at as isize;
    for &(from, to, length) in edits {
        let (from, to, length) = (from as isize, to as isize, length as isize);
        let start = from - shift;
        if at < start {
            break;
        }
        if at < start + length {
            return (from + (at - start).min(to - from - 1)) as usize;
        }
        shift += to - from - length;
    }
    (at + shift) as usize
}

pub fn input_offset(steps: &[Vec<Edit>], mut at: usize) -> usize {
    for s in steps.iter().rev() {
        at = step_input(s, at);
    }
    at
}

/// How positions in the parsed text map back to the source as written.
pub struct Origin {
    pub parsed: Vec<u16>,
    pub find: Box<dyn Fn(usize) -> (Vec<u16>, usize)>,
}

thread_local! {
    static ORIGIN: RefCell<Vec<Origin>> = const { RefCell::new(Vec::new()) };
}

/// Report positions in the original source while `run` parses.
pub fn locating_in<T>(next: Origin, run: impl FnOnce() -> T) -> T {
    ORIGIN.with(|o| o.borrow_mut().push(next));
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            ORIGIN.with(|o| {
                o.borrow_mut().pop();
            });
        }
    }
    let _pop = Pop;
    run()
}

/// Maps only when `src` is exactly the text being parsed.
pub fn written(src: &[u16], offset: usize) -> (Vec<u16>, usize) {
    ORIGIN.with(|o| {
        let o = o.borrow();
        match o.last() {
            Some(origin) if origin.parsed == src => (origin.find)(offset),
            _ => (src.to_vec(), offset),
        }
    })
}

pub struct Location {
    pub line: usize,
    pub column: usize,
    pub snippet: String,
}

fn index_of(s: &[u16], c: u16, from: usize) -> Option<usize> {
    s.iter().skip(from).position(|&x| x == c).map(|p| p + from)
}

pub fn locate(src: &[u16], offset: usize) -> Location {
    // Keep EOF errors on the last content line.
    let end = trim_end16(src).len();
    let (src, offset) = if end > 0 && offset >= end {
        let (s, o) = written(src, end - 1);
        (s, o + 1)
    } else {
        written(src, offset)
    };
    let mut line = 1;
    let mut line_start = 0;
    let mut i = index_of(&src, b'\n' as u16, 0);
    while let Some(k) = i {
        if k >= offset {
            break;
        }
        line += 1;
        line_start = k + 1;
        i = index_of(&src, b'\n' as u16, k + 1);
    }
    let line_end = index_of(&src, b'\n' as u16, offset.min(src.len())).unwrap_or(src.len());
    let column = offset - line_start + 1;
    let text = String::from_utf16_lossy(&src[line_start.min(src.len())..line_end.max(line_start.min(src.len()))]);
    Location { line, column, snippet: format!("{}\n{}^", text, "-".repeat(column - 1)) }
}

fn trim_end16(s: &[u16]) -> &[u16] {
    let mut e = s.len();
    while e > 0 && crate::js::is_space_u(s[e - 1] as u32) {
        e -= 1;
    }
    &s[..e]
}

pub fn syntax_error(type_: &str, src: &[u16], offset: usize, detail: &str, lexical: bool) -> PeleError {
    let loc = locate(src, offset);
    let message = if lexical {
        format!("Lexical error on line {}. Unrecognized text.\n{}", loc.line, loc.snippet)
    } else {
        format!("Parse error on line {}:\n{}\n{}", loc.line, loc.snippet, detail)
    };
    PeleError {
        message,
        code: PeleErrorCode::Syntax,
        type_: Some(type_.to_string()),
        mermaid_type: None,
        line: loc.line,
        column: loc.column,
        snippet: loc.snippet,
    }
}
