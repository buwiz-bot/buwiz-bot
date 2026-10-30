// Port of src/diagrams/flowchart/lexer.ts. Reads UTF-16 code units, as upstream does, so token
// offsets (and so error columns) match. Upstream's sticky regexes are matched by hand below.

use super::tokens as t;
use crate::util::chars::{is_space, is_word};
use crate::util::unicode::is_unicode_letter;

pub struct Tokens {
    pub types: Vec<u8>,
    pub texts: Vec<Vec<u16>>,
    pub starts: Vec<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum S {
    Initial,
    String,
    MdString,
    AccTitle,
    AccDescr,
    AccDescrMultiline,
    Dir,
    Text,
    EllipseText,
    TrapText,
    EdgeText,
    ThickEdgeText,
    DottedEdgeText,
    Click,
    Callbackname,
    Callbackargs,
    ShapeData,
    ShapeDataStr,
}

const NODE_CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!\"#$%&'*+.`?\\_/";

fn node_char(c: u32) -> bool {
    c < 128 && NODE_CHARS.as_bytes().contains(&(c as u8))
}

fn is_line_end(c: u32) -> bool {
    c == 10 || c == 13 || c == 0x2028 || c == 0x2029
}

/// Which link pattern to match.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Re {
    Link,
    StartLink,
    ThickLink,
    ThickStart,
    DottedLink,
    DottedStart,
    InvisLink,
}

struct Lx<'a> {
    src: &'a [u16],
    n: usize,
}

impl Lx<'_> {
    fn c(&self, i: usize) -> u32 {
        if i < self.n {
            self.src[i] as u32
        } else {
            u32::MAX
        }
    }

    fn ws(&self, mut q: usize) -> usize {
        while q < self.n && is_space(self.c(q)) {
            q += 1;
        }
        q
    }

    fn starts_with(&self, word: &str, p: usize) -> bool {
        let w: Vec<u16> = word.encode_utf16().collect();
        p + w.len() <= self.n && self.src[p..p + w.len()] == w[..]
    }

    fn lit(&self, q: usize, word: &str) -> Option<usize> {
        if self.starts_with(word, q) {
            Some(q + word.len())
        } else {
            None
        }
    }

    fn link(&self, re: Re, p: usize) -> Option<usize> {
        let mut q = self.ws(p);
        if re == Re::InvisLink {
            let s = q;
            while self.c(q) == 126 {
                q += 1;
            }
            if q - s < 3 {
                return None;
            }
            return Some(self.ws(q));
        }
        let pre = self.c(q);
        if pre == 120 || pre == 111 || pre == 60 {
            q += 1;
        }
        let end = match re {
            Re::Link | Re::ThickLink => {
                let ch = if re == Re::Link { 45 } else { 61 };
                let s = q;
                while self.c(q) == ch {
                    q += 1;
                }
                let run = q - s;
                let last = self.c(q);
                if run >= 2 && (last == ch || last == 120 || last == 111 || last == 62) {
                    q + 1
                } else if run >= 3 {
                    q
                } else {
                    return None;
                }
            }
            Re::StartLink => self.lit(q, "--")?,
            Re::ThickStart => self.lit(q, "==")?,
            Re::DottedLink => {
                if self.c(q) == 45 && self.c(q + 1) == 46 {
                    q += 1;
                }
                let s = q;
                while self.c(q) == 46 {
                    q += 1;
                }
                if q == s || self.c(q) != 45 {
                    return None;
                }
                q += 1;
                let k = self.c(q);
                if k == 120 || k == 111 || k == 62 {
                    q += 1;
                }
                q
            }
            Re::DottedStart => self.lit(q, "-.")?,
            Re::InvisLink => unreachable!(),
        };
        Some(self.ws(end))
    }

    /// /word\s*:\s*/ and the like: `word`, blanks, an optional `sep`, blanks.
    fn word_sep(&self, p: usize, word: &str, sep: u32) -> Option<usize> {
        let q = self.ws(self.lit(p, word)?);
        if self.c(q) != sep {
            return None;
        }
        Some(self.ws(q + 1))
    }

    /// /word\s+/
    fn word_space(&self, p: usize, word: &str) -> Option<usize> {
        let q = self.lit(p, word)?;
        let e = self.ws(q);
        if e == q {
            None
        } else {
            Some(e)
        }
    }

    /// /(?:\r?\n)*\s*\n/: the whitespace from p, up to and with its last line break.
    fn nodir(&self, p: usize) -> Option<usize> {
        let e = self.ws(p);
        (p..e).rev().find(|&i| self.c(i) == 10).map(|i| i + 1)
    }

    fn dir(&self, p: usize) -> Option<usize> {
        let q = self.ws(p);
        for d in ["LR", "RL", "TB", "BT", "TD", "BR"] {
            if self.starts_with(d, q) && !is_word(self.c(q + 2)) {
                return Some(q + 2);
            }
        }
        None
    }

    fn dir_sym(&self, p: usize) -> Option<usize> {
        let q = self.ws(p);
        let c = self.c(q);
        if c == 60 || c == 62 || c == 94 {
            Some(q + 1)
        } else {
            None
        }
    }

    fn dir_v(&self, p: usize) -> Option<usize> {
        let q = self.ws(p);
        if self.c(q) == 118 && !is_word(self.c(q + 1)) {
            Some(q + 1)
        } else {
            None
        }
    }

    /// /.*direction\s+XX[^\n]*/y
    fn direction(&self, p: usize, dir: &str) -> Option<usize> {
        let mut line_end = p;
        while line_end < self.n && !is_line_end(self.c(line_end)) {
            line_end += 1;
        }
        let mut j = line_end;
        while j > p {
            j -= 1;
            if !self.starts_with("direction", j) {
                continue;
            }
            let q = j + 9;
            let e = self.ws(q);
            if e == q || !self.starts_with(dir, e) {
                continue;
            }
            let mut k = e + 2;
            while k < self.n && self.c(k) != 10 {
                k += 1;
            }
            return Some(k);
        }
        None
    }

    /// /(?:\r?\n)+/y
    fn newline(&self, p: usize) -> Option<usize> {
        let mut q = p;
        loop {
            if self.c(q) == 10 {
                q += 1;
            } else if self.c(q) == 13 && self.c(q + 1) == 10 {
                q += 2;
            } else {
                break;
            }
        }
        if q > p {
            Some(q)
        } else {
            None
        }
    }

    /// /[^\S\n\r]+/y
    fn space(&self, p: usize) -> Option<usize> {
        let mut q = p;
        while q < self.n && is_space(self.c(q)) && self.c(q) != 10 && self.c(q) != 13 {
            q += 1;
        }
        if q > p {
            Some(q)
        } else {
            None
        }
    }

    fn index_of(&self, ch: u32, from: usize) -> Option<usize> {
        (from..self.n).find(|&i| self.c(i) == ch)
    }
}

pub fn tokenize(src: &[u16]) -> Tokens {
    let lx = Lx { src, n: src.len() };
    let n = src.len();
    let mut tk = Tokens { types: Vec::new(), texts: Vec::new(), starts: Vec::new() };
    let mut stack: Vec<S> = Vec::new();
    let mut state = S::Initial;
    let mut p: usize = 0;
    let mut first_graph = true;
    let find = |word: &str, from: usize| -> Option<usize> {
        let w: Vec<u16> = word.encode_utf16().collect();
        if from >= n || w.len() > n {
            return None;
        }
        (from..=n - w.len()).find(|&i| src[i..i + w.len()] == w[..])
    };
    let mut dir_next: Option<usize> = find("direction", 0);
    let mut at_next: Option<usize> = lx.index_of(64, 0);
    let mut line_end: isize = -1;
    let mut dir_failed: usize = 0;
    let mut run_end: usize = 0;
    let mut run_at: isize = -1;
    let mut ws_from: isize = -1;
    let mut ws_to: isize = -1;

    macro_rules! emit {
        ($ty:expr, $s:expr, $e:expr) => {{
            let (s, e) = ($s, $e);
            tk.types.push($ty);
            tk.texts.push(src[s..e].to_vec());
            tk.starts.push(s);
            p = e;
        }};
    }
    macro_rules! push {
        ($s:expr) => {{
            stack.push(state);
            state = $s;
        }};
    }
    macro_rules! pop {
        () => {{
            if let Some(s) = stack.pop() {
                state = s;
            }
        }};
    }
    // First character after any run of whitespace starting at q, skipping one optional x, o or <.
    macro_rules! skip_ws {
        ($q:expr) => {{
            let q = $q as isize;
            let mut j = q;
            if q >= ws_from && q <= ws_to {
                j = ws_to;
            } else if is_space(lx.c(q as usize)) {
                while (j as usize) < n && is_space(lx.c(j as usize)) {
                    j += 1;
                }
                ws_from = q;
                ws_to = j;
            }
            j as usize
        }};
    }
    macro_rules! link_char {
        ($q:expr) => {{
            let j = skip_ws!($q);
            let d = lx.c(j);
            if d == 120 || d == 111 || d == 60 {
                lx.c(j + 1)
            } else {
                d
            }
        }};
    }
    macro_rules! invis_ahead {
        ($q:expr) => {{
            let j = skip_ws!($q);
            lx.c(j) == 126
        }};
    }
    macro_rules! quote {
        () => {{
            if lx.c(p + 1) == 96 {
                push!(S::MdString);
                p += 2;
            } else {
                push!(S::String);
                p += 1;
            }
        }};
    }
    macro_rules! invis_link {
        () => {{
            if !invis_ahead!(p) {
                false
            } else if let Some(e) = lx.link(Re::InvisLink, p) {
                emit!(t::LINK, p, e);
                true
            } else {
                false
            }
        }};
    }
    macro_rules! opener {
        ($c:expr) => {{
            let c: u32 = $c;
            let d = lx.c(p + 1);
            if c == 40 {
                if d == 45 {
                    push!(S::EllipseText);
                    emit!(t::ELLIPSE_START, p, p + 2);
                } else if d == 91 {
                    push!(S::Text);
                    emit!(t::STADIUMSTART, p, p + 2);
                } else if d == 40 && lx.c(p + 2) == 40 {
                    push!(S::Text);
                    emit!(t::DOUBLECIRCLESTART, p, p + 3);
                } else {
                    push!(S::Text);
                    emit!(t::PS, p, p + 1);
                }
                true
            } else if c == 91 {
                if d == 91 {
                    push!(S::Text);
                    emit!(t::SUBROUTINESTART, p, p + 2);
                } else if d == 40 {
                    push!(S::Text);
                    emit!(t::CYLINDERSTART, p, p + 2);
                } else if d == 47 {
                    push!(S::TrapText);
                    emit!(t::TRAPSTART, p, p + 2);
                } else if d == 92 {
                    push!(S::TrapText);
                    emit!(t::INVTRAPSTART, p, p + 2);
                } else {
                    push!(S::Text);
                    emit!(t::SQS, p, p + 1);
                }
                true
            } else if c == 124 {
                push!(S::Text);
                emit!(t::PIPE, p, p + 1);
                true
            } else if c == 123 {
                push!(S::Text);
                emit!(t::DIAMOND_START, p, p + 1);
                true
            } else {
                false
            }
        }};
    }
    macro_rules! graph {
        ($len:expr) => {{
            if first_graph {
                first_graph = false;
                push!(S::Dir);
            }
            emit!(t::GRAPH, p, p + $len);
        }};
    }
    macro_rules! node_string {
        () => {{
            let mut q = p;
            while q < n {
                let ch = lx.c(q);
                if node_char(ch) {
                    q += 1;
                } else if ch == 45 && q + 1 < n {
                    let d = lx.c(q + 1);
                    if d == 62 || d == 45 || d == 46 {
                        break;
                    }
                    q += 1;
                } else {
                    break;
                }
            }
            if q == p {
                false
            } else {
                emit!(t::NODE_STRING, p, q);
                true
            }
        }};
    }
    macro_rules! link {
        ($c:expr) => {{
            let c: u32 = $c;
            let d = link_char!(p);
            let mut done = false;
            if d == 45 {
                if let Some(e) = lx.link(Re::Link, p) {
                    emit!(t::LINK, p, e);
                    done = true;
                } else if let Some(e) = lx.link(Re::StartLink, p) {
                    push!(S::EdgeText);
                    emit!(t::START_LINK, p, e);
                    done = true;
                } else if let Some(e) = lx.link(Re::DottedLink, p) {
                    emit!(t::LINK, p, e);
                    done = true;
                } else if let Some(e) = lx.link(Re::DottedStart, p) {
                    push!(S::DottedEdgeText);
                    emit!(t::START_LINK, p, e);
                    done = true;
                }
            } else if d == 61 {
                if let Some(e) = lx.link(Re::ThickLink, p) {
                    emit!(t::LINK, p, e);
                    done = true;
                } else if let Some(e) = lx.link(Re::ThickStart, p) {
                    push!(S::ThickEdgeText);
                    emit!(t::START_LINK, p, e);
                    done = true;
                }
            } else if d == 46 {
                if let Some(e) = lx.link(Re::DottedLink, p) {
                    emit!(t::LINK, p, e);
                    done = true;
                }
            }
            done || (c != 45 && c != 61 && c != 46 && invis_link!())
        }};
    }
    macro_rules! kw {
        ($word:expr) => {
            lx.starts_with($word, p) && !is_word(lx.c(p + $word.len()))
        };
    }
    macro_rules! edge_text {
        ($c:expr, $re:expr, $a:expr, $b:expr) => {{
            let c: u32 = $c;
            let a: u32 = $a;
            let b: u32 = $b;
            if c == 34 {
                quote!();
                true
            } else {
                let d = link_char!(p);
                let mut done = false;
                if d == a || d == b {
                    if let Some(e) = lx.link($re, p) {
                        pop!();
                        emit!(t::LINK, p, e);
                        done = true;
                    }
                }
                if done {
                    true
                } else if c == a && (a != 45 || lx.c(p + 1) == 45) {
                    false
                } else {
                    let mut q = p + 1;
                    while q < n {
                        let ch = lx.c(q);
                        if ch == 34 {
                            break;
                        }
                        if ch == a {
                            if a != 45 || lx.c(q + 1) == 45 {
                                break;
                            }
                        } else if ch == 120 || ch == 111 || ch == 60 || ch == b || is_space(ch) {
                            let dd = link_char!(q);
                            if (dd == a || dd == b) && lx.link($re, q).is_some() {
                                break;
                            }
                        }
                        q += 1;
                    }
                    emit!(t::EDGE_TEXT, p, q);
                    true
                }
            }
        }};
    }

    loop {
        if p >= n {
            let ty = match state {
                S::Initial => Some(t::EOF),
                S::AccTitle => Some(t::acc_title_value),
                S::AccDescr => Some(t::acc_descr_value),
                S::AccDescrMultiline => Some(t::acc_descr_multiline_value),
                S::Click => Some(t::CLICK),
                S::Callbackname => Some(t::CALLBACKNAME),
                S::Callbackargs => Some(t::CALLBACKARGS),
                _ => None,
            };
            if let Some(ty) = ty {
                emit!(ty, n, n);
            }
            break;
        }

        let c = lx.c(p);
        let mut ok = true;

        match state {
            S::Initial => {
                ok = 'initial: {
                    match c {
                        97 => {
                            if lx.starts_with("acc", p) {
                                if let Some(e) = lx.word_sep(p, "accTitle", 58) {
                                    push!(S::AccTitle);
                                    emit!(t::acc_title, p, e);
                                    break 'initial true;
                                }
                                if let Some(e) = lx.word_sep(p, "accDescr", 58) {
                                    push!(S::AccDescr);
                                    emit!(t::acc_descr, p, e);
                                    break 'initial true;
                                }
                                if let Some(e) = lx.word_sep(p, "accDescr", 123) {
                                    push!(S::AccDescrMultiline);
                                    p = e;
                                    break 'initial true;
                                }
                            }
                        }
                        64 => {
                            if lx.c(p + 1) == 123 {
                                push!(S::ShapeData);
                                tk.types.push(t::SHAPE_DATA);
                                tk.texts.push(Vec::new());
                                tk.starts.push(p);
                                p += 2;
                                break 'initial true;
                            }
                        }
                        99 => {
                            if let Some(e) = lx.word_space(p, "call") {
                                push!(S::Callbackname);
                                p = e;
                                break 'initial true;
                            }
                            if kw!("classDef") {
                                emit!(t::CLASSDEF, p, p + 8);
                                break 'initial true;
                            }
                            if kw!("class") {
                                emit!(t::CLASS, p, p + 5);
                                break 'initial true;
                            }
                            if let Some(e) = lx.word_space(p, "click") {
                                push!(S::Click);
                                p = e;
                                break 'initial true;
                            }
                        }
                        34 => {
                            quote!();
                            break 'initial true;
                        }
                        115 => {
                            if kw!("style") {
                                emit!(t::STYLE, p, p + 5);
                                break 'initial true;
                            }
                            if kw!("swimlane-beta") {
                                graph!(13);
                                break 'initial true;
                            }
                            if kw!("subgraph") {
                                emit!(t::subgraph, p, p + 8);
                                break 'initial true;
                            }
                        }
                        100 => {
                            if kw!("default") {
                                emit!(t::DEFAULT, p, p + 7);
                                break 'initial true;
                            }
                        }
                        108 => {
                            if kw!("linkStyle") {
                                emit!(t::LINKSTYLE, p, p + 9);
                                break 'initial true;
                            }
                        }
                        105 => {
                            if kw!("interpolate") {
                                emit!(t::INTERPOLATE, p, p + 11);
                                break 'initial true;
                            }
                        }
                        104 => {
                            if lx.starts_with("href", p) && is_space(lx.c(p + 4)) {
                                emit!(t::HREF, p, p + 5);
                                break 'initial true;
                            }
                        }
                        102 => {
                            if kw!("flowchart-elk") {
                                graph!(13);
                                break 'initial true;
                            }
                            if kw!("flowchart") {
                                graph!(9);
                                break 'initial true;
                            }
                        }
                        103 => {
                            if kw!("graph") {
                                graph!(5);
                                break 'initial true;
                            }
                        }
                        101 => {
                            if kw!("end") {
                                let e = lx.ws(p + 3);
                                emit!(t::end, p, e);
                                break 'initial true;
                            }
                        }
                        95 => {
                            for (word, len) in [("_self", 5), ("_blank", 6), ("_parent", 7), ("_top", 4)] {
                                if kw!(word) {
                                    emit!(t::LINK_TARGET, p, p + len);
                                    break 'initial true;
                                }
                            }
                        }
                        _ => {}
                    }

                    if dir_next.is_some() && p >= dir_failed {
                        if dir_next.is_some_and(|d| d < p) {
                            dir_next = find("direction", p);
                        }
                        if let Some(dn) = dir_next {
                            if line_end < p as isize {
                                let mut k = p;
                                while k < n && !is_line_end(lx.c(k)) {
                                    k += 1;
                                }
                                line_end = k as isize;
                            }
                            if (dn as isize) < line_end {
                                for (i, d) in ["TB", "BT", "RL", "LR", "TD"].iter().enumerate() {
                                    if let Some(e) = lx.direction(p, d) {
                                        emit!(t::direction_tb + i as u8, p, e);
                                        break 'initial true;
                                    }
                                }
                                // No direction statement starts anywhere in the rest of this line either.
                                dir_failed = line_end as usize;
                            }
                        }
                    }

                    // A link id is a run of non-space characters up to its last `@` that is not followed by `{` or `"`.
                    if at_next.is_some() && c != 34 && !is_space(c) {
                        if p >= run_end {
                            if at_next.is_some_and(|a| a < p) {
                                at_next = lx.index_of(64, p);
                            }
                            let mut q = p;
                            while q < n {
                                let ch = lx.c(q);
                                if ch == 34 || is_space(ch) {
                                    break;
                                }
                                q += 1;
                            }
                            run_end = q;
                            run_at = -1;
                            if let Some(an) = at_next {
                                if an < q {
                                    let mut k = q.min(n - 1) as isize - 1;
                                    while k >= an as isize {
                                        if lx.c(k as usize) == 64 {
                                            let after = lx.c(k as usize + 1);
                                            if after != 123 && after != 34 {
                                                run_at = k;
                                                break;
                                            }
                                        }
                                        k -= 1;
                                    }
                                }
                            }
                        }
                        if run_at > p as isize {
                            emit!(t::LINK_ID, p, run_at as usize + 1);
                            break 'initial true;
                        }
                    }

                    if (48..=57).contains(&c) {
                        let mut q = p + 1;
                        while q < n && (48..=57).contains(&lx.c(q)) {
                            q += 1;
                        }
                        emit!(t::NUM, p, q);
                        break 'initial true;
                    }

                    match c {
                        35 => {
                            emit!(t::BRKT, p, p + 1);
                            return_true()
                        }
                        58 => {
                            if lx.c(p + 1) == 58 && lx.c(p + 2) == 58 {
                                emit!(t::STYLE_SEPARATOR, p, p + 3);
                            } else {
                                emit!(t::COLON, p, p + 1);
                            }
                            true
                        }
                        38 => {
                            emit!(t::AMP, p, p + 1);
                            true
                        }
                        59 => {
                            emit!(t::SEMI, p, p + 1);
                            true
                        }
                        44 => {
                            emit!(t::COMMA, p, p + 1);
                            true
                        }
                        42 => {
                            emit!(t::MULT, p, p + 1);
                            true
                        }
                        120 | 111 => link!(c) || node_string!(),
                        60 => {
                            if link!(c) {
                                true
                            } else {
                                emit!(t::TAGSTART, p, p + 1);
                                true
                            }
                        }
                        45 => {
                            if link!(c) || node_string!() {
                                true
                            } else {
                                emit!(t::MINUS, p, p + 1);
                                true
                            }
                        }
                        61 => link!(c),
                        46 => link!(c) || node_string!(),
                        126 => invis_link!(),
                        40 => opener!(c),
                        91 => {
                            if lx.c(p + 1) == 124 {
                                emit!(t::VERTEX_WITH_PROPS_START, p, p + 2);
                                true
                            } else {
                                opener!(c)
                            }
                        }
                        124 | 123 => opener!(c),
                        62 => {
                            push!(S::Text);
                            emit!(t::TAGEND, p, p + 1);
                            true
                        }
                        94 => {
                            emit!(t::UP, p, p + 1);
                            true
                        }
                        92 => {
                            if lx.c(p + 1) == 124 {
                                emit!(t::SEP, p, p + 2);
                                true
                            } else {
                                node_string!()
                            }
                        }
                        118 => {
                            if !is_word(lx.c(p + 1)) {
                                emit!(t::DOWN, p, p + 1);
                                true
                            } else {
                                node_string!()
                            }
                        }
                        _ => {
                            if is_space(c) {
                                if link!(c) {
                                    true
                                } else if let Some(e) = lx.newline(p) {
                                    emit!(t::NEWLINE, p, e);
                                    true
                                } else if let Some(e) = lx.space(p) {
                                    emit!(t::SPACE, p, e);
                                    true
                                } else {
                                    false
                                }
                            } else if c < 128 {
                                node_string!()
                            } else if is_unicode_letter(c) {
                                emit!(t::UNICODE_TEXT, p, p + 1);
                                true
                            } else {
                                false
                            }
                        }
                    }
                };
            }

            S::String => {
                if c == 34 {
                    if lx.c(p + 1) == 96 {
                        push!(S::MdString);
                        p += 2;
                    } else {
                        pop!();
                        p += 1;
                    }
                } else {
                    let e = lx.index_of(34, p).unwrap_or(n);
                    emit!(t::STR, p, e);
                }
            }

            S::MdString => {
                if c == 96 {
                    if lx.c(p + 1) == 34 {
                        pop!();
                        p += 2;
                    } else {
                        ok = false;
                    }
                } else if c == 34 {
                    quote!();
                } else {
                    let mut q = p + 1;
                    while q < n && lx.c(q) != 96 && lx.c(q) != 34 {
                        q += 1;
                    }
                    emit!(t::MD_STR, p, q);
                }
            }

            S::AccTitle | S::AccDescr => {
                let e = lx.index_of(10, p).unwrap_or(n);
                let ty = if state == S::AccTitle { t::acc_title_value } else { t::acc_descr_value };
                pop!();
                emit!(ty, p, e);
            }

            S::AccDescrMultiline => {
                if c == 125 {
                    pop!();
                    p += 1;
                } else {
                    let e = lx.index_of(125, p).unwrap_or(n);
                    emit!(t::acc_descr_multiline_value, p, e);
                }
            }

            S::Dir => {
                if c == 34 {
                    quote!();
                } else if let Some(e) = lx.nodir(p) {
                    pop!();
                    emit!(t::NODIR, p, e);
                } else if let Some(e) = lx.dir(p).or_else(|| lx.dir_sym(p)).or_else(|| lx.dir_v(p)) {
                    pop!();
                    emit!(t::DIR, p, e);
                } else {
                    ok = invis_link!() || opener!(c);
                }
            }

            S::Text => {
                let d = lx.c(p + 1);
                match c {
                    34 => quote!(),
                    40 | 91 | 123 => {
                        opener!(c);
                    }
                    93 => {
                        pop!();
                        if d == 41 {
                            emit!(t::STADIUMEND, p, p + 2);
                        } else if d == 93 {
                            emit!(t::SUBROUTINEEND, p, p + 2);
                        } else {
                            emit!(t::SQE, p, p + 1);
                        }
                    }
                    41 => {
                        pop!();
                        if d == 93 {
                            emit!(t::CYLINDEREND, p, p + 2);
                        } else if d == 41 && lx.c(p + 2) == 41 {
                            emit!(t::DOUBLECIRCLEEND, p, p + 3);
                        } else {
                            emit!(t::PE, p, p + 1);
                        }
                    }
                    124 => {
                        pop!();
                        emit!(t::PIPE, p, p + 1);
                    }
                    125 => {
                        pop!();
                        emit!(t::DIAMOND_STOP, p, p + 1);
                    }
                    _ => {
                        if !((c == 126 || is_space(c)) && invis_link!()) {
                            let mut q = p + 1;
                            while q < n {
                                let ch = lx.c(q);
                                if matches!(ch, 91 | 93 | 40 | 41 | 123 | 125 | 124 | 34) {
                                    break;
                                }
                                q += 1;
                            }
                            emit!(t::TEXT, p, q);
                        }
                    }
                }
            }

            S::EllipseText => {
                let d = lx.c(p + 1);
                if c == 34 {
                    quote!();
                } else if (c == 126 || is_space(c)) && invis_link!() {
                } else if (c == 45 || c == 47 || c == 41) && d == 41 {
                    pop!();
                    emit!(t::ELLIPSE_END, p, p + 2);
                } else if c == 40 || c == 91 || c == 123 {
                    opener!(c);
                } else if c == 41 || c == 93 || c == 125 {
                    ok = false;
                } else {
                    let mut q = p + 1;
                    while q < n {
                        let ch = lx.c(q);
                        if matches!(ch, 34 | 40 | 41 | 91 | 93 | 123 | 125) {
                            break;
                        }
                        if (ch == 45 || ch == 47) && lx.c(q + 1) == 41 {
                            break;
                        }
                        if (ch == 126 || is_space(ch)) && invis_ahead!(q) && lx.link(Re::InvisLink, q).is_some() {
                            break;
                        }
                        q += 1;
                    }
                    emit!(t::TEXT, p, q);
                }
            }

            S::TrapText => {
                let d = lx.c(p + 1);
                if c == 34 {
                    quote!();
                } else if (c == 126 || is_space(c)) && invis_link!() {
                } else {
                    match c {
                        40 => {
                            if d == 45 || d == 91 || (d == 40 && lx.c(p + 2) == 40) {
                                opener!(c);
                            } else if d == 93 {
                                pop!();
                                emit!(t::TRAPEND, p, p + 2);
                            } else {
                                opener!(c);
                            }
                        }
                        91 | 123 => {
                            opener!(c);
                        }
                        92 => {
                            if d == 93 {
                                pop!();
                                emit!(t::TRAPEND, p, p + 2);
                            } else {
                                emit!(t::TEXT, p, p + 1);
                            }
                        }
                        47 => {
                            if d == 93 {
                                pop!();
                                emit!(t::INVTRAPEND, p, p + 2);
                            } else {
                                emit!(t::TEXT, p, p + 1);
                            }
                        }
                        93 | 41 => {
                            if d == 93 {
                                pop!();
                                emit!(t::TRAPEND, p, p + 2);
                            } else {
                                ok = false;
                            }
                        }
                        125 => ok = false,
                        _ => {
                            if (c == 63 || c == 61) && d == 93 {
                                pop!();
                                emit!(t::TRAPEND, p, p + 2);
                            } else {
                                let mut q = p + 1;
                                while q < n {
                                    let ch = lx.c(q);
                                    if matches!(ch, 92 | 91 | 93 | 40 | 41 | 123 | 125 | 47) {
                                        break;
                                    }
                                    q += 1;
                                }
                                emit!(t::TEXT, p, q);
                            }
                        }
                    }
                }
            }

            S::EdgeText => ok = edge_text!(c, Re::Link, 45, u32::MAX - 1),
            S::ThickEdgeText => ok = edge_text!(c, Re::ThickLink, 61, u32::MAX - 1),
            S::DottedEdgeText => ok = edge_text!(c, Re::DottedLink, 46, 45),

            S::Click => {
                if c == 34 {
                    quote!();
                } else if is_space(c) {
                    pop!();
                    p += 1;
                } else {
                    let mut q = p + 1;
                    while q < n && !is_space(lx.c(q)) {
                        q += 1;
                    }
                    emit!(t::CLICK, p, q);
                }
            }

            S::Callbackname => {
                if c == 40 {
                    // /\(\s*\)/y
                    let q = lx.ws(p + 1);
                    let empty_call = if lx.c(q) == 41 { Some(q + 1) } else { None };
                    pop!();
                    match empty_call {
                        Some(e) => p = e,
                        None => {
                            push!(S::Callbackargs);
                            p += 1;
                        }
                    }
                } else {
                    let e = lx.index_of(40, p).unwrap_or(n);
                    emit!(t::CALLBACKNAME, p, e);
                }
            }

            S::Callbackargs => {
                if c == 41 {
                    pop!();
                    p += 1;
                } else {
                    let e = lx.index_of(41, p).unwrap_or(n);
                    emit!(t::CALLBACKARGS, p, e);
                }
            }

            S::ShapeData => {
                if c == 34 {
                    push!(S::ShapeDataStr);
                    emit!(t::SHAPE_DATA, p, p + 1);
                } else if c == 125 {
                    pop!();
                    p += 1;
                } else if c == 94 {
                    ok = false;
                } else {
                    let mut q = p + 1;
                    while q < n {
                        let ch = lx.c(q);
                        if ch == 125 || ch == 94 || ch == 34 {
                            break;
                        }
                        q += 1;
                    }
                    emit!(t::SHAPE_DATA, p, q);
                }
            }

            S::ShapeDataStr => {
                if c == 34 {
                    pop!();
                    emit!(t::SHAPE_DATA, p, p + 1);
                } else {
                    let e = lx.index_of(34, p).unwrap_or(n);
                    // .replace(/\n\s*/g, '<br/>')
                    let mut text: Vec<u16> = Vec::new();
                    let mut i = p;
                    while i < e {
                        if src[i] == 10 {
                            text.extend("<br/>".encode_utf16());
                            i += 1;
                            while i < e && is_space(src[i] as u32) {
                                i += 1;
                            }
                            continue;
                        }
                        text.push(src[i]);
                        i += 1;
                    }
                    tk.types.push(t::SHAPE_DATA);
                    tk.texts.push(text);
                    tk.starts.push(p);
                    p = e;
                }
            }
        }

        if !ok {
            tk.types.push(t::ERROR);
            tk.texts.push(Vec::new());
            tk.starts.push(p);
            return tk;
        }
    }

    tk.types.push(t::END);
    tk.texts.push(Vec::new());
    tk.starts.push(n);
    tk
}

#[inline]
fn return_true() -> bool {
    true
}
