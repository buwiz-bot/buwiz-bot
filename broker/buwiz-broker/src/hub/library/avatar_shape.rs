//! avatar-shape.mjs: what a bot's avatar body may be, and the avatar
//! designer's candidate logic. A body is a kit shape (the app's mark engine
//! draws 13 of them) or "custom": an SVG outline in a 0 0 100 100 box, y
//! down, drawn with M/L/C/Q/Z only. The checks, their error texts and the
//! retry → drop → top-up → kit fallback rules are Node's, step for step.

use std::future::Future;

use serde_json::{json, Map, Value};

use crate::js;

/// The kit: name → what it looks like.
pub const KIT: [(&str, &str); 13] = [
    ("blob", "a ball"),
    ("cloud", "a cloud"),
    ("square", "a rounded square"),
    ("sparkle", "a 4-point star"),
    ("clover", "4 leaves"),
    ("heart", "a heart"),
    ("flower", "a star flower"),
    ("teardrop", "a teardrop"),
    ("tablet", "a wide pill"),
    ("wedge", "a rounded triangle"),
    ("house", "a house"),
    ("star", "a 6-point star"),
    ("hex", "a hexagon"),
];
pub const OUTLINE_MAX: usize = 4000;

/// The designer's system prompt: the file Node's `DESIGN_SYSTEM` reads.
pub fn design_system() -> &'static str {
    const TEXT: &str = include_str!("../../../../../server/src/templates/avatar-design-system.txt");
    TEXT.strip_suffix('\n').unwrap_or(TEXT)
}

/// The rich designer's extra instructions (Node's `RICH_SYSTEM`).
pub fn rich_system() -> &'static str {
    const TEXT: &str = include_str!("../../../../../server/src/templates/avatar-rich-system.txt");
    TEXT.strip_suffix('\n').unwrap_or(TEXT)
}

/// Picture mode's style prompt (avatar-design.mjs `PICTURE_STYLE`).
pub fn picture_style() -> &'static str {
    const TEXT: &str = include_str!("../../../../../server/src/templates/avatar-picture-style.txt");
    TEXT.strip_suffix('\n').unwrap_or(TEXT)
}

pub const LAYERS_MAX: usize = 8;

pub fn is_kit(shape: &str) -> bool {
    KIT.iter().any(|(k, _)| *k == shape)
}

// curve sampling for the geometry checks
const STEPS_C: usize = 16;
const STEPS_Q: usize = 10;

type P = [f64; 2];

/// What `flattenPath` returns.
pub struct Flat {
    pub points: Vec<P>,
    /// every point the data names, control points too
    pub coords: Vec<P>,
    pub closed: bool,
    /// the points the mark engine samples it to (a solid's marks)
    pub sampled: usize,
}

/// How finely the mark engine samples a solid's marks (app/src/mark/solids.rs).
const MARK_STEP: f64 = 2.0;

/// What that sampler makes of a segment `len` long.
fn mark_steps(len: f64) -> usize {
    (len / MARK_STEP).ceil().clamp(2.0, 4096.0) as usize
}

/// `d.match(/[MLCQZmlcqz]|-?(?:\d+\.?\d*|\.\d+)/g)`.
fn tokens(d: &str) -> Vec<String> {
    let b: Vec<char> = d.chars().collect();
    let digit = |i: usize| b.get(i).is_some_and(|c| c.is_ascii_digit());
    // a number at i (no sign): its end
    let number = |i: usize| -> Option<usize> {
        if digit(i) {
            let mut j = i;
            while digit(j) {
                j += 1;
            }
            if b.get(j) == Some(&'.') {
                j += 1;
                while digit(j) {
                    j += 1;
                }
            }
            Some(j)
        } else if b.get(i) == Some(&'.') && digit(i + 1) {
            let mut j = i + 1;
            while digit(j) {
                j += 1;
            }
            Some(j)
        } else {
            None
        }
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if "MLCQZmlcqz".contains(b[i]) {
            out.push(b[i].to_string());
            i += 1;
            continue;
        }
        if b[i] == '-' {
            if let Some(j) = number(i + 1) {
                out.push(b[i..j].iter().collect());
                i = j;
                continue;
            }
        }
        if let Some(j) = number(i) {
            out.push(b[i..j].iter().collect());
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

/// `flattenPath(d)`: M/L/C/Q/Z path data (absolute and relative, implicit
/// repeats) as a polygon, or the syntax error.
pub fn flatten_path(d: &str) -> Result<Flat, String> {
    let toks = tokens(d);
    let kept: usize = d.chars().filter(|c| !js::is_space(*c) && *c != ',').map(char::len_utf16).sum();
    let joined: usize = toks.iter().map(|t| t.len()).sum();
    if joined != kept || has_run_on(d) {
        return Err("outline has a malformed number".into());
    }
    let letter = |t: &str| t.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
    let (mut points, mut coords): (Vec<P>, Vec<P>) = (Vec::new(), Vec::new());
    let (mut cur, mut start): (P, P) = ([0.0, 0.0], [0.0, 0.0]);
    let (mut cmd, mut subpaths, mut closed, mut i) = (String::new(), 0, false, 0usize);
    let mut sampled = 0usize;
    macro_rules! num {
        () => {{
            match toks.get(i) {
                Some(t) if !letter(t) => {
                    i += 1;
                    t.parse::<f64>().unwrap_or(f64::NAN)
                }
                _ => return Err(format!("outline command {cmd} is missing numbers")),
            }
        }};
    }
    macro_rules! pt {
        ($rel:expr) => {{
            let x = num!();
            let y = num!();
            let p = if $rel { [cur[0] + x, cur[1] + y] } else { [x, y] };
            coords.push(p);
            p
        }};
    }
    while i < toks.len() {
        let t = toks[i].clone();
        if letter(&t) {
            cmd = t;
            i += 1;
        } else if cmd.is_empty() {
            return Err("outline must start with M".into());
        } else if cmd == "Z" || cmd == "z" {
            return Err("outline has numbers after Z".into());
        } else if cmd == "M" {
            cmd = "L".into();
        } else if cmd == "m" {
            cmd = "l".into();
        }
        let up = cmd.to_ascii_uppercase();
        let rel = cmd != up;
        if points.is_empty() && up != "M" {
            return Err("outline must start with M".into());
        }
        match up.as_str() {
            "M" => {
                if subpaths > 0 {
                    return Err("outline must be one shape (a single M)".into());
                }
                subpaths += 1;
                cur = pt!(rel);
                start = cur;
                points.push(cur);
                sampled += 1;
            }
            "Z" => {
                closed = true;
                let back = (start[0] - cur[0]).hypot(start[1] - cur[1]);
                if back > 0.01 {
                    sampled += mark_steps(back);
                }
                cur = start;
            }
            _ => {
                if closed {
                    return Err("outline must be one shape (nothing after Z)".into());
                }
                if up == "L" {
                    let from = cur;
                    cur = pt!(rel);
                    points.push(cur);
                    sampled += mark_steps((cur[0] - from[0]).hypot(cur[1] - from[1]));
                } else {
                    let p0 = cur;
                    let n_ctl = if up == "C" { 3 } else { 2 };
                    let mut ctl = Vec::new();
                    for _ in 0..n_ctl {
                        ctl.push(pt!(rel));
                    }
                    let mut len = 0.0;
                    let mut from = p0;
                    for q in &ctl {
                        len += (q[0] - from[0]).hypot(q[1] - from[1]);
                        from = *q;
                    }
                    sampled += mark_steps(len);
                    let n = if up == "C" { STEPS_C } else { STEPS_Q };
                    for s in 1..=n {
                        let u = s as f64 / n as f64;
                        let v = 1.0 - u;
                        let (a, b, c, e) = if up == "C" { (v * v * v, 3.0 * v * v * u, 3.0 * v * u * u, u * u * u) } else { (v * v, 2.0 * v * u, u * u, 0.0) };
                        let q: [P; 4] = if up == "C" { [p0, ctl[0], ctl[1], ctl[2]] } else { [p0, ctl[0], ctl[1], [0.0, 0.0]] };
                        points.push([a * q[0][0] + b * q[1][0] + c * q[2][0] + e * q[3][0], a * q[0][1] + b * q[1][1] + c * q[2][1] + e * q[3][1]]);
                    }
                    cur = *ctl.last().unwrap();
                }
            }
        }
    }
    if points.is_empty() {
        return Err("outline is empty".into());
    }
    Ok(Flat { points, coords, closed, sampled })
}

/// `/\.\d*\./`: a number running into the next ("1.2.3").
fn has_run_on(d: &str) -> bool {
    let b = d.as_bytes();
    (0..b.len()).any(|i| {
        if b[i] != b'.' {
            return false;
        }
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        j < b.len() && b[j] == b'.'
    })
}

fn area_centroid(poly: &[P]) -> (f64, P) {
    let (mut a, mut cx, mut cy) = (0.0, 0.0, 0.0);
    for k in 0..poly.len() {
        let ([x0, y0], [x1, y1]) = (poly[k], poly[(k + 1) % poly.len()]);
        let f = x0 * y1 - x1 * y0;
        a += f;
        cx += (x0 + x1) * f;
        cy += (y0 + y1) * f;
    }
    ((a / 2.0).abs(), if a != 0.0 { [cx / (3.0 * a), cy / (3.0 * a)] } else { poly[0] })
}

// even-odd point in polygon
fn inside(poly: &[P], x: f64, y: f64) -> bool {
    let mut hit = false;
    let mut j = poly.len() - 1;
    for k in 0..poly.len() {
        let ([xi, yi], [xj, yj]) = (poly[k], poly[j]);
        if (yi > y) != (yj > y) && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi {
            hit = !hit;
        }
        j = k;
    }
    hit
}

// How often a ray from c at angle th crosses the polygon's edges.
fn crossings(poly: &[P], c: P, th: f64) -> usize {
    let (dx, dy) = (th.cos(), th.sin());
    let mut n = 0;
    for k in 0..poly.len() {
        let ([ax, ay], [bx, by]) = (poly[k], poly[(k + 1) % poly.len()]);
        let (ex, ey) = (bx - ax, by - ay);
        let den = dx * ey - dy * ex;
        if den.abs() < 1e-12 {
            continue;
        }
        let (wx, wy) = (ax - c[0], ay - c[1]);
        let t = (wx * ey - wy * ex) / den;
        let u = (wx * dy - wy * dx) / den;
        if t > 1e-9 && (0.0..1.0).contains(&u) {
            n += 1;
        }
    }
    n
}

// Parts thinner than ~6 units: rasterize at 1 unit, open with a disk of
// radius 2.5 (erode, then dilate); a sizeable removed patch is thin.
const THIN_RADIUS: f64 = 2.5;
const THIN_CELLS: usize = 40;
fn thin_part(poly: &[P]) -> bool {
    const W: usize = 112;
    const O: f64 = -6.0;
    let at = |x: usize, y: usize| y * W + x;
    let mut fill = vec![0u8; W * W];
    for y in 0..W {
        for x in 0..W {
            fill[at(x, y)] = inside(poly, x as f64 + O + 0.5, y as f64 + O + 0.5) as u8;
        }
    }
    let r = THIN_RADIUS.ceil() as i64;
    let mut disk = Vec::new();
    for y in -r..=r {
        for x in -r..=r {
            if ((x * x + y * y) as f64) <= THIN_RADIUS * THIN_RADIUS {
                disk.push((x, y));
            }
        }
    }
    let morph = |src: &[u8], all: bool| -> Vec<u8> {
        let mut out = vec![0u8; W * W];
        let r = r as usize;
        for y in r..W - r {
            for x in r..W - r {
                let on = |&(a, b): &(i64, i64)| src[at((x as i64 + a) as usize, (y as i64 + b) as usize)] != 0;
                out[at(x, y)] = if all { disk.iter().all(on) } else { disk.iter().any(on) } as u8;
            }
        }
        out
    };
    let opened = morph(&morph(&fill, true), false);
    let mut seen = vec![0u8; W * W];
    for s in 0..W * W {
        if fill[s] == 0 || opened[s] != 0 || seen[s] != 0 {
            continue;
        }
        let mut size = 0;
        let mut stack = vec![s];
        seen[s] = 1;
        while let Some(q) = stack.pop() {
            size += 1;
            let (x, y) = ((q % W) as i64, (q / W) as i64);
            for (nx, ny) in [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)] {
                if nx < 0 || ny < 0 || nx >= W as i64 || ny >= W as i64 {
                    continue;
                }
                let n = at(nx as usize, ny as usize);
                if fill[n] != 0 && opened[n] == 0 && seen[n] == 0 {
                    seen[n] = 1;
                    stack.push(n);
                }
            }
        }
        if size > THIN_CELLS {
            return true;
        }
    }
    false
}

/// `x.toFixed(2)`: a tie rounds up, as V8 does.
fn to_fixed2(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let exact = format!("{:.60}", x.abs());
    let (_, frac) = exact.split_once('.').unwrap_or((&exact, ""));
    let tie = frac.len() > 2 && frac[2..].starts_with('5') && frac[3..].bytes().all(|b| b == b'0');
    let y = if tie { f64::from_bits(x.abs().to_bits() + 1) } else { x.abs() };
    let s = format!("{y:.2}");
    if x < 0.0 && s.bytes().any(|b| b != b'0' && b != b'.') {
        format!("-{s}")
    } else {
        s
    }
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// The problems with an outline's path data, empty when it is a good one.
/// `None` is anything not a string.
pub fn outline_errors(outline: Option<&str>) -> Vec<String> {
    outline_errors_from(outline, 2000.0)
}

/// `outlineErrors(outline, {minArea})`: a part's outline may be smaller.
pub fn outline_errors_from(outline: Option<&str>, min_area: f64) -> Vec<String> {
    let Some(outline) = outline.filter(|o| !o.trim_matches(js::is_space).is_empty()) else { return vec!["a custom shape needs an outline".into()] };
    if utf16_len(outline) > OUTLINE_MAX {
        return vec![format!("outline is longer than {OUTLINE_MAX} characters")];
    }
    if !outline.chars().all(is_outline_char) {
        return vec!["outline may hold only M/L/C/Q/Z commands and numbers".into()];
    }
    let f = match flatten_path(outline) {
        Ok(f) => f,
        Err(e) => return vec![e],
    };
    let mut errs: Vec<String> = Vec::new();
    let within = |v: f64| (-5.0..=105.0).contains(&v);
    if f.coords.iter().any(|[x, y]| !(within(*x) && within(*y))) {
        errs.push("outline coordinates must stay within -5..105".into());
    }
    let pts = &f.points;
    let mut poly: Vec<P> = (0..pts.len()).filter(|&k| k == 0 || (pts[k][0] - pts[k - 1][0]).hypot(pts[k][1] - pts[k - 1][1]) > 1e-6).map(|k| pts[k]).collect();
    let (first, last) = (poly[0], *poly.last().unwrap());
    let ends = (first[0] - last[0]).hypot(first[1] - last[1]);
    if !f.closed && ends > 1.0 {
        errs.push("outline must be closed (end with Z)".into());
    }
    if ends <= 1e-6 && poly.len() > 1 {
        poly.pop();
    }
    if poly.len() < 3 {
        errs.push("outline encloses no area".into());
        return errs;
    }
    let (area, c) = area_centroid(&poly);
    if area < min_area {
        // Math.round: half up
        errs.push(format!("outline covers {}% of the box; it must cover at least {}%", js::number((area / 100.0 + 0.5).floor()), js::number(min_area / 100.0)));
    }
    let fold = |init: f64, f: fn(f64, f64) -> f64, k: usize| poly.iter().map(|p| p[k]).fold(init, f);
    let w = fold(f64::NEG_INFINITY, f64::max, 0) - fold(f64::INFINITY, f64::min, 0);
    let h = fold(f64::NEG_INFINITY, f64::max, 1) - fold(f64::INFINITY, f64::min, 1);
    let aspect = if h != 0.0 && !h.is_nan() { w / h } else { f64::INFINITY };
    if !((0.4..=2.5).contains(&aspect)) {
        errs.push(format!("outline's width/height is {}; it must be between 0.4 and 2.5", to_fixed2(aspect)));
    }
    if area >= 1.0 {
        // offset so no ray runs exactly through a vertex of a tidy outline
        let bad = (0..64).filter(|&k| crossings(&poly, c, ((k as f64 + 0.37) * 2.0 * std::f64::consts::PI) / 64.0) != 1).count();
        if bad > 0 {
            errs.push("outline must be star-shaped from its centre (every ray from the centre crosses the edge once): no crossings, holes, or bays that fold back".into());
        } else if errs.is_empty() && thin_part(&poly) {
            // (the raster covers the allowed box only: run it on an otherwise good one)
            errs.push("outline has a part thinner than ~6 units".into());
        }
    }
    errs
}

fn is_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_outline_char(c: char) -> bool {
    "MLCQZmlcqz0123456789.,-".contains(c) || js::is_space(c)
}

/// `layerErrors(layers, eyes)`: a rich avatar's painted layers (up to 8
/// closed paths in the outline's box, each with a fill and an opacity
/// 0..1) and the eyes' colour. `None` is undefined.
pub fn layer_errors(layers: Option<&Value>, eyes: Option<&Value>, outline: Option<&str>) -> Vec<String> {
    layer_errors_with(layers, eyes, outline, true)
}

/// `layerErrors(…, {checkEyes})`: a part's layers have no eyes.
pub fn layer_errors_with(layers: Option<&Value>, eyes: Option<&Value>, outline: Option<&str>, check_eyes: bool) -> Vec<String> {
    let mut errs: Vec<String> = Vec::new();
    let Some(layers) = layers.and_then(Value::as_array).filter(|l| !l.is_empty()) else { return vec!["a rich avatar needs at least one layer (the face)".into()] };
    if layers.len() > LAYERS_MAX {
        errs.push(format!("at most {LAYERS_MAX} layers"));
    }
    let empty = Map::new();
    for (k, l) in layers.iter().enumerate() {
        let n = format!("layer {}", k + 1);
        let l = match l {
            Value::Object(o) => o,
            Value::Array(_) => &empty,
            _ => {
                errs.push(format!("{n} must be {{path, fill}}"));
                continue;
            }
        };
        if !l.get("fill").and_then(Value::as_str).is_some_and(is_color) {
            errs.push(format!("{n}'s fill must be #RRGGBB"));
        }
        if let Some(o) = l.get("opacity") {
            if !o.as_f64().is_some_and(|o| (0.0..=1.0).contains(&o)) {
                errs.push(format!("{n}'s opacity must be 0..1"));
            }
        }
        if l.get("over").is_some_and(|o| !o.is_boolean()) {
            errs.push(format!("{n}'s over must be true or false"));
        }
        let Some(d) = l.get("path").and_then(Value::as_str).filter(|d| !d.trim_matches(js::is_space).is_empty()) else {
            errs.push(format!("{n} needs a path"));
            continue;
        };
        if utf16_len(d) > OUTLINE_MAX {
            errs.push(format!("{n}'s path is longer than {OUTLINE_MAX} characters"));
            continue;
        }
        if !d.chars().all(is_outline_char) {
            errs.push(format!("{n}'s path may hold only M/L/C/Q/Z commands and numbers"));
            continue;
        }
        let f = match flatten_path(d) {
            Ok(f) => f,
            Err(e) => {
                errs.push(format!("{n}: {e}"));
                continue;
            }
        };
        let within = |v: f64| (-5.0..=105.0).contains(&v);
        if f.coords.iter().any(|[x, y]| !(within(*x) && within(*y))) {
            errs.push(format!("{n}'s coordinates must stay within -5..105"));
        }
        if f.points.len() < 3 || area_centroid(&f.points).0.abs() < 4.0 {
            errs.push(format!("{n} encloses no area"));
        }
    }
    if check_eyes {
        errs.extend(eye_errors(eyes, outline));
    }
    errs
}

pub const PARTS_MAX: usize = 6;
// a part may be a small piece (a brim, an antler): 2% of the box, not 20%
const PART_MIN_AREA: f64 = 200.0;
/// a part's half-depth, as a multiple of the head's
pub const PART_DEPTH: (f64, f64) = (0.05, 1.6);

/// `partErrors(parts)`: up to 6 pieces of their own `[{outline, color,
/// layers?, z?}]` in the head's box, each outline checked as the head's
/// (from 2% of the box), its layers as the head's (no eyes), `z` -1..1.
pub fn part_errors(parts: &Value) -> Vec<String> {
    let Some(parts) = parts.as_array() else { return vec!["parts must be a list".into()] };
    let mut errs: Vec<String> = Vec::new();
    if parts.len() > PARTS_MAX {
        errs.push(format!("at most {PARTS_MAX} parts"));
    }
    for (k, p) in parts.iter().enumerate() {
        let n = format!("part {}", k + 1);
        let Some(p) = p.as_object() else {
            errs.push(format!("{n} must be {{outline, color}}"));
            continue;
        };
        if !p.get("color").and_then(Value::as_str).is_some_and(is_color) {
            errs.push(format!("{n}'s color must be #RRGGBB"));
        }
        let outline = p.get("outline").and_then(Value::as_str);
        errs.extend(outline_errors_from(outline, PART_MIN_AREA).into_iter().map(|e| format!("{n}: {e}")));
        if let Some(ls) = p.get("layers") {
            match ls.as_array() {
                None => errs.push(format!("{n}'s layers must be a list")),
                Some(a) if !a.is_empty() => errs.extend(layer_errors_with(Some(ls), None, outline, false).into_iter().map(|e| format!("{n}: {e}"))),
                _ => {}
            }
        }
        if p.contains_key("z") && !p.get("z").filter(|v| v.is_number()).and_then(Value::as_f64).is_some_and(|z| (-1.0..=1.0).contains(&z)) {
            errs.push(format!("{n}'s z must be -1..1"));
        }
        if p.contains_key("d") && !p.get("d").filter(|v| v.is_number()).and_then(Value::as_f64).is_some_and(|d| (PART_DEPTH.0..=PART_DEPTH.1).contains(&d)) {
            errs.push(format!("{n}'s d must be {}..{}", js::number(PART_DEPTH.0), js::number(PART_DEPTH.1)));
        }
    }
    errs
}

pub const SOLIDS_MAX: usize = 10;
const SOLID_PROFILE: usize = 40;
const SOLID_PATH: usize = 12;
const SOLID_BRANCHES: usize = 6;
const SOLID_MARKS: usize = 8;
const SOLID_EDGE: usize = 24;
const SOLID_SCALLOPS: usize = 18;
const SOLID_LOBES: usize = 32;
const SOLID_MARK_POINTS: usize = 1000;
const SOLID_ALL_MARK_POINTS: usize = 2000;
// the mark engine's cost of a solid: faces it cuts at most (app/src/mark/solids.rs)
const SOLID_FACES: usize = 8000;
const SOLID_ROW: f64 = 5.0;
const SOLID_COLS: usize = 68;
const SOLID_SHELL_FACES: usize = 6 * 72;
const SOLID_LOBE_FACES: usize = 84;
const SOLID_TUBE_STEPS: usize = 3;
const SOLID_TUBE_SIDES: usize = 8;
const SOLID_BOX: (f64, f64) = (-60.0, 160.0);
const SOLID_RADIUS: f64 = 60.0;

/// `numList(v, n)`: a list of `n` finite numbers.
fn num_list(v: Option<&Value>, n: usize) -> Option<Vec<f64>> {
    let a = v?.as_array()?;
    if a.len() != n {
        return None;
    }
    a.iter().map(|x| x.as_f64().filter(|x| x.is_finite())).collect()
}

fn in_box(v: f64) -> bool {
    (SOLID_BOX.0..=SOLID_BOX.1).contains(&v)
}

/// `tubePathErrors(path, what)`: a tube's or branch's `[[x, y, z, radius], ...]`.
fn tube_path_errors(path: Option<&Value>, what: &str) -> Vec<String> {
    let Some(a) = path.and_then(Value::as_array) else { return vec![format!("{what} must be a list of [x, y, z, radius]")] };
    if !(2..=SOLID_PATH).contains(&a.len()) {
        return vec![format!("{what} needs 2 to {SOLID_PATH} points")];
    }
    let mut errs = Vec::new();
    for p in a {
        match num_list(Some(p), 4) {
            None => errs.push(format!("{what}'s point must be a list of 4 numbers")),
            Some(p) if !p[..3].iter().all(|v| in_box(*v)) || !(0.0..=SOLID_RADIUS).contains(&p[3]) => errs.push(format!("{what}'s point is out of range")),
            _ => {}
        }
    }
    errs
}

/// `solidCost(s)`: what a checked solid costs the renderer, in faces at most.
fn solid_cost(s: &Map<String, Value>) -> usize {
    let list = |k: &str| s.get(k).and_then(Value::as_array);
    let tube = |p: &Vec<Value>| ((p.len() - 1) * SOLID_TUBE_STEPS + 3) * SOLID_TUBE_SIDES;
    match s.get("kind").and_then(Value::as_str) {
        Some("lathe") => {
            let p: Vec<Vec<f64>> = list("profile").map(|l| l.iter().filter_map(|q| num_list(Some(q), 2)).collect()).unwrap_or_default();
            let rows: usize = p.windows(2).map(|w| (((w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]) / SOLID_ROW).ceil() as usize).max(1)).sum();
            rows * SOLID_COLS
        }
        Some("shell") => SOLID_SHELL_FACES + list("lobes").and_then(|l| l.first()).and_then(Value::as_u64).map_or(0, |n| n as usize * SOLID_LOBE_FACES),
        _ => list("path").map_or(0, tube) + list("branches").map_or(0, |b| b.iter().filter_map(Value::as_array).map(tube).sum()),
    }
}

/// A whole number in `lo..=hi`.
fn whole(v: f64, lo: usize, hi: usize) -> bool {
    v.fract() == 0.0 && (lo as f64..=hi as f64).contains(&v)
}

/// `solidErrors(solids)`: up to 10 pieces of real 3D in head space, `[{kind,
/// color, marks?, ...}]`, under the limits that keep a bad recipe from
/// stalling the renderer (app/src/mark/solids.rs checks the same).
pub fn solid_errors(solids: &Value) -> Vec<String> {
    let Some(solids) = solids.as_array() else { return vec!["solids must be a list".into()] };
    let mut errs: Vec<String> = Vec::new();
    if solids.len() > SOLIDS_MAX {
        errs.push(format!("at most {SOLIDS_MAX} solids"));
    }
    let mut cost = 0;
    for (k, s) in solids.iter().enumerate() {
        let n = format!("solid {}", k + 1);
        let Some(s) = s.as_object() else {
            errs.push(format!("{n} must be {{kind, color}}"));
            continue;
        };
        let before = errs.len();
        let mut mark_points = 0;
        if !s.get("color").and_then(Value::as_str).is_some_and(is_color) {
            errs.push(format!("{n}'s color must be #RRGGBB"));
        }
        match s.get("kind").and_then(Value::as_str) {
            Some("lathe") => {
                match num_list(s.get("at"), 3) {
                    None => errs.push(format!("{n}'s at must be [x, y, z]")),
                    Some(a) if !a.iter().all(|v| in_box(*v)) => errs.push(format!("{n}'s at is out of range")),
                    _ => {}
                }
                if s.contains_key("tilt") && !num_list(s.get("tilt"), 3).is_some_and(|t| t.iter().all(|t| t.abs() <= 180.0)) {
                    errs.push(format!("{n}'s tilt must be [x, y, z] degrees, -180..180"));
                }
                if s.contains_key("oval") && !s.get("oval").and_then(Value::as_f64).is_some_and(|o| (0.3..=3.0).contains(&o)) {
                    errs.push(format!("{n}'s oval must be 0.3..3"));
                }
                if s.contains_key("teeth") && !num_list(s.get("teeth"), 2).is_some_and(|t| whole(t[0], 2, SOLID_SCALLOPS) && (0.0..=1.0).contains(&t[1])) {
                    errs.push(format!("{n}'s teeth must be [2..{SOLID_SCALLOPS}, 0..1]"));
                }
                match s.get("profile").and_then(Value::as_array).filter(|p| (2..=SOLID_PROFILE).contains(&p.len())) {
                    None => errs.push(format!("{n}'s profile needs 2 to {SOLID_PROFILE} points")),
                    Some(p) => {
                        let q: Option<Vec<Vec<f64>>> = p.iter().map(|q| num_list(Some(q), 2).filter(|q| (0.0..=2.0 * SOLID_RADIUS).contains(&q[0]) && q[1].abs() <= 2.0 * SOLID_RADIUS)).collect();
                        match q {
                            None => errs.push(format!("{n}'s profile points must be [radius, height], in range")),
                            Some(q) => {
                                let (a, b) = (&q[0], &q[q.len() - 1]);
                                if !(a == b || (a[0] == 0.0 && b[0] == 0.0)) {
                                    errs.push(format!("{n}'s profile must run axis to axis, or back to its start"));
                                }
                            }
                        }
                    }
                }
            }
            Some("shell") => {
                if s.contains_key("offset") && !s.get("offset").and_then(Value::as_f64).is_some_and(|o| (0.0..=SOLID_RADIUS).contains(&o)) {
                    errs.push(format!("{n}'s offset must be 0..{}", js::number(SOLID_RADIUS)));
                }
                match s.get("edge").and_then(Value::as_array).filter(|e| (1..=SOLID_EDGE).contains(&e.len())) {
                    None => errs.push(format!("{n}'s edge needs 1 to {SOLID_EDGE} points")),
                    Some(e) => {
                        let q: Option<Vec<Vec<f64>>> = e.iter().map(|q| num_list(Some(q), 2).filter(|q| q[0].abs() <= 180.0 && (5.0..=175.0).contains(&q[1]))).collect();
                        match q {
                            None => errs.push(format!("{n}'s edge points must be [-180..180, 5..175]")),
                            Some(q) if q.windows(2).any(|w| w[0][0] >= w[1][0]) => errs.push(format!("{n}'s edge azimuths must rise")),
                            _ => {}
                        }
                    }
                }
                if s.contains_key("scallop") && !num_list(s.get("scallop"), 3).is_some_and(|c| whole(c[0], 2, SOLID_SCALLOPS) && (0.0..=60.0).contains(&c[1]) && (c[2] == 0.0 || c[2] == 1.0)) {
                    errs.push(format!("{n}'s scallop must be [2..{SOLID_SCALLOPS}, 0..60, 0 or 1]"));
                }
                if s.contains_key("lobes") && !num_list(s.get("lobes"), 3).is_some_and(|l| whole(l[0], 1, SOLID_LOBES) && (1.0..=SOLID_RADIUS).contains(&l[1]) && l[2].abs() <= SOLID_RADIUS) {
                    let r = js::number(SOLID_RADIUS);
                    errs.push(format!("{n}'s lobes must be [1..{SOLID_LOBES}, 1..{r}, -{r}..{r}]"));
                }
            }
            Some("tube") => {
                errs.extend(tube_path_errors(s.get("path"), &format!("{n}'s path")));
                if let Some(b) = s.get("branches") {
                    match b.as_array() {
                        None => errs.push(format!("{n}'s branches must be a list")),
                        Some(b) => {
                            if b.len() > SOLID_BRANCHES {
                                errs.push(format!("{n} has more than {SOLID_BRANCHES} branches"));
                            }
                            for p in b {
                                errs.extend(tube_path_errors(Some(p), &format!("{n}'s branch")));
                            }
                        }
                    }
                }
            }
            _ => errs.push(format!("{n}'s kind must be lathe, shell or tube")),
        }
        if let Some(m) = s.get("marks") {
            match m.as_array() {
                None => errs.push(format!("{n}'s marks must be a list")),
                Some(marks) => {
                    if marks.len() > SOLID_MARKS {
                        errs.push(format!("{n} has more than {SOLID_MARKS} marks"));
                    }
                    for (i, m) in marks.iter().enumerate() {
                        errs.extend(mark_errors(m, &format!("{n} mark {}", i + 1), &mut mark_points));
                    }
                    if mark_points > SOLID_ALL_MARK_POINTS {
                        errs.push(format!("{n}'s marks sample to {mark_points} points, at most {SOLID_ALL_MARK_POINTS}"));
                    }
                }
            }
        }
        if errs.len() == before {
            cost += solid_cost(s) + mark_points;
        }
    }
    if cost > SOLID_FACES {
        errs.push(format!("the solids cost {cost} faces, at most {SOLID_FACES}"));
    }
    errs
}

/// One mark of a solid: a closed path in its chart, u -180..180 round it
/// and v 0..100 along it, with a fill.
fn mark_errors(m: &Value, n: &str, points: &mut usize) -> Vec<String> {
    let Some(m) = m.as_object() else { return vec![format!("{n} must be {{path, fill}}")] };
    let mut errs = Vec::new();
    if !m.get("fill").and_then(Value::as_str).is_some_and(is_color) {
        errs.push(format!("{n}'s fill must be #RRGGBB"));
    }
    let Some(d) = m.get("path").and_then(Value::as_str).filter(|d| !d.trim_matches(js::is_space).is_empty()) else {
        errs.push(format!("{n} needs a path"));
        return errs;
    };
    if utf16_len(d) > OUTLINE_MAX {
        errs.push(format!("{n}'s path is longer than {OUTLINE_MAX} characters"));
        return errs;
    }
    if !d.chars().all(is_outline_char) {
        errs.push(format!("{n}'s path may hold only M/L/C/Q/Z commands and numbers"));
        return errs;
    }
    let f = match flatten_path(d) {
        Ok(f) => f,
        Err(e) => {
            errs.push(format!("{n}: {e}"));
            return errs;
        }
    };
    if f.coords.iter().any(|[u, v]| !((-180.0..=180.0).contains(u) && (-5.0..=105.0).contains(v))) {
        errs.push(format!("{n}'s coordinates must stay within -180..180, -5..105"));
    }
    if f.points.len() < 3 || area_centroid(&f.points).0 < 0.25 {
        errs.push(format!("{n} encloses no area"));
    }
    if f.sampled > SOLID_MARK_POINTS {
        errs.push(format!("{n} samples to {} points, at most {SOLID_MARK_POINTS}", f.sampled));
    }
    *points += f.sampled;
    errs
}

/// `solidOf(s)`: a solid as a bot stores it, the fields of its kind and
/// nothing else.
pub fn solid_of(s: &Value) -> Value {
    let keys: &[&str] = match s.get("kind").and_then(Value::as_str) {
        Some("lathe") => &["at", "tilt", "oval", "teeth", "profile"],
        Some("shell") => &["offset", "edge", "scallop", "lobes"],
        Some("tube") => &["path", "branches"],
        _ => return s.clone(),
    };
    let keep = |from: &Value, names: &[&str]| -> Map<String, Value> {
        names.iter().filter_map(|k| from.get(*k).map(|v| ((*k).to_owned(), v.clone()))).collect()
    };
    let mut o = keep(s, &["kind", "color"]);
    o.extend(keep(s, keys));
    if let Some(m) = s.get("marks").and_then(Value::as_array) {
        o.insert("marks".into(), m.iter().map(|m| Value::Object(keep(m, &["path", "fill"]))).collect());
    }
    Value::Object(o)
}

/// The smallest and largest eye size multiplier, and the thickest rim.
pub const EYE_SCALE: (f64, f64) = (0.6, 2.2);
pub const RIM_MAX: f64 = 1.2;

/// The outline's bounding box `[x0, y0, x1, y1]`, or `None` when it doesn't parse.
fn outline_box(outline: Option<&str>) -> Option<[f64; 4]> {
    let pts = flatten_path(outline?).ok()?.points;
    let f = |init: f64, g: fn(f64, f64) -> f64, k: usize| pts.iter().map(|p| p[k]).fold(init, g);
    Some([f(f64::INFINITY, f64::min, 0), f(f64::INFINITY, f64::min, 1), f(f64::NEG_INFINITY, f64::max, 0), f(f64::NEG_INFINITY, f64::max, 1)])
}

/// `eyeErrors(eyes, outline)`: a colour "#RRGGBB", or the recipe's
/// placement `{c, at?, gap?, s?, rim?, rw?}` (`at` inside the outline's
/// box, `s` 0.6..2.2, `rw` 0..1.2). `None` is undefined.
pub fn eye_errors(eyes: Option<&Value>, outline: Option<&str>) -> Vec<String> {
    let e = match eyes {
        Some(Value::String(c)) => return if is_color(c) { vec![] } else { vec!["eyes must be #RRGGBB".into()] },
        Some(Value::Object(o)) => o,
        _ => return vec!["eyes must be #RRGGBB or {c, at?, gap?, s?, rim?, rw?}".into()],
    };
    let mut errs: Vec<String> = Vec::new();
    let num = |k: &str| e.get(k).and_then(|v| if v.is_number() { v.as_f64() } else { None });
    if !e.get("c").and_then(Value::as_str).is_some_and(is_color) {
        errs.push("eyes.c must be #RRGGBB".into());
    }
    if let Some(at) = e.get("at") {
        match at.as_array().filter(|a| a.len() == 2 && a.iter().all(Value::is_number)) {
            None => errs.push("eyes.at must be [x, y]".into()),
            Some(a) => {
                let (x, y) = (a[0].as_f64().unwrap_or(f64::NAN), a[1].as_f64().unwrap_or(f64::NAN));
                if let Some(b) = outline_box(outline) {
                    if !(x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3]) {
                        errs.push("eyes.at must sit inside the outline's box".into());
                    }
                }
            }
        }
    }
    if e.contains_key("gap") && !num("gap").is_some_and(|g| g > 0.0 && g <= 100.0) {
        errs.push("eyes.gap must be a number 0..100".into());
    }
    if e.contains_key("s") && !num("s").is_some_and(|v| (EYE_SCALE.0..=EYE_SCALE.1).contains(&v)) {
        errs.push(format!("eyes.s must be {}..{}", js::number(EYE_SCALE.0), js::number(EYE_SCALE.1)));
    }
    if e.contains_key("rim") && !e.get("rim").and_then(Value::as_str).is_some_and(is_color) {
        errs.push("eyes.rim must be #RRGGBB".into());
    }
    if e.contains_key("rw") && !num("rw").is_some_and(|v| (0.0..=RIM_MAX).contains(&v)) {
        errs.push(format!("eyes.rw must be 0..{}", js::number(RIM_MAX)));
    }
    if e.contains_key("shut") && !matches!(e.get("shut").and_then(Value::as_str), Some("l" | "r")) {
        errs.push("eyes.shut must be l or r".into());
    }
    errs
}

/// `validateAvatar({shape, color, outline, layers, eyes})`: `None` for a
/// value that is not a string (`layers`, `eyes`: for undefined). Layers
/// (with eyes) make it a rich avatar, which only a custom outline can be.
pub fn validate_avatar(shape: Option<&str>, color: Option<&str>, outline: Option<&str>, layers: Option<&Value>, eyes: Option<&Value>, parts: Option<&Value>, solids: Option<&Value>) -> Vec<String> {
    let mut errs = Vec::new();
    if shape != Some("custom") && !shape.is_some_and(is_kit) {
        errs.push(format!("shape must be one of {} or custom", KIT.iter().map(|k| k.0).collect::<Vec<_>>().join(", ")));
    }
    if !color.is_some_and(is_color) {
        errs.push("color must be #RRGGBB".into());
    }
    if shape == Some("custom") {
        errs.extend(outline_errors(outline));
    }
    let rich = layers.and_then(Value::as_array).is_some_and(|l| !l.is_empty()) || eyes.is_some();
    if rich {
        if shape != Some("custom") {
            errs.push("only a custom outline can have layers".into());
        } else {
            errs.extend(layer_errors(layers, eyes, outline));
        }
    }
    if let Some(parts) = parts {
        if shape != Some("custom") {
            errs.push("only a custom outline can have parts".into());
        } else {
            errs.extend(part_errors(parts));
        }
    }
    if let Some(solids) = solids {
        if shape != Some("custom") {
            errs.push("only a custom outline can have solids".into());
        } else {
            errs.extend(solid_errors(solids));
            if solids.as_array().is_some_and(|s| !s.is_empty()) && !rich {
                errs.push("solids need a rich avatar: at least one layer (the face)".into());
            }
        }
    }
    errs
}

/// A candidate as the designer returns it.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub name: String,
    pub shape: String,
    pub color: String,
    /// for a custom shape only
    pub outline: Option<String>,
    /// a rich candidate's painted layers `[{path, fill, opacity?}]` (custom only)
    pub layers: Option<Vec<Value>>,
    /// a rich candidate's pieces `[{outline, color, layers?, z?}]` (custom only)
    pub parts: Option<Vec<Value>>,
    /// a rich candidate's real 3D pieces (custom only)
    pub solids: Option<Vec<Value>>,
    /// a rich candidate's eyes, a colour or a placement (custom only)
    pub eyes: Option<Value>,
    pub why: String,
}

impl Candidate {
    pub fn errors(&self) -> Vec<String> {
        let layers = self.layers.clone().map(Value::Array);
        let eyes = self.eyes.clone();
        let parts = self.parts.clone().map(Value::Array);
        let solids = self.solids.clone().map(Value::Array);
        validate_avatar(Some(&self.shape), Some(&self.color), self.outline.as_deref(), layers.as_ref(), eyes.as_ref(), parts.as_ref(), solids.as_ref())
    }

    /// `{name, shape, color, outline?, why}`
    pub fn json(&self) -> Value {
        let mut o = Map::new();
        o.insert("name".into(), json!(self.name));
        o.insert("shape".into(), json!(self.shape));
        o.insert("color".into(), json!(self.color));
        if let Some(x) = &self.outline {
            o.insert("outline".into(), json!(x));
        }
        if let Some(x) = &self.layers {
            o.insert("layers".into(), Value::Array(x.clone()));
        }
        if let Some(x) = &self.eyes {
            o.insert("eyes".into(), x.clone());
        }
        if let Some(x) = &self.parts {
            o.insert("parts".into(), Value::Array(x.clone()));
        }
        if let Some(x) = &self.solids {
            o.insert("solids".into(), Value::Array(x.clone()));
        }
        o.insert("why".into(), json!(self.why));
        Value::Object(o)
    }
}

/// `designRequest({prompt, name, image, rich})`: the user turn's text.
pub fn design_request(prompt: &str, name: &str, image: bool, rich: bool) -> String {
    if rich {
        return rich_request(prompt, name, image);
    }
    let mut lines = Vec::new();
    if !name.is_empty() {
        lines.push(format!("The bot's name: {name}"));
    }
    lines.push(if prompt.is_empty() {
        "The request: none. Make the picture's subject into an avatar, in its own main colour, and name each candidate after it.".to_owned()
    } else {
        format!("The request (what the bot is or does, or how it should look): {prompt}")
    });
    lines.push(if image {
        "The image attached is the subject. Candidate 1 MUST be that subject made into an avatar: shape \"custom\", an outline tracing the subject's silhouette (simplified to the rules), in #FFFFFF unless the request asks for colour (then the subject's main colour). The request says what to keep or change. Candidates 2 and 3 are other takes: another custom outline or a fitting kit shape.".to_owned()
    } else {
        "Candidates may be kit shapes or custom outlines; at least one must be a kit shape. Use a custom outline when the bot has an obvious emblem (a receipt, a leaf, a cat's head).".to_owned()
    });
    lines.join("\n")
}

/// `richRequest({prompt, name, image})`: a rich design's user text.
pub fn rich_request(prompt: &str, name: &str, image: bool) -> String {
    let mut lines = Vec::new();
    if !name.is_empty() {
        lines.push(format!("The bot's name: {name}"));
    }
    lines.push(if prompt.is_empty() { "The request: none. Make the picture's subject into a rich avatar.".to_owned() } else { format!("The request: {prompt}") });
    lines.push(
        if image { "The image attached is the subject. Keep its identity: its main colour on the shell, its hair or head outline, ears or headgear, its face colour, its eye colour." } else { "Design the character the request describes." }.to_owned(),
    );
    lines.push("Give exactly 3 rich candidates (different takes), each with shape \"custom\", outline, color (the shell), layers and eyes.".to_owned());
    lines.join("\n")
}

fn js_trim(s: &str) -> &str {
    s.trim_matches(js::is_space)
}

/// `String(x || "")`
fn or_empty(v: Option<&Value>) -> String {
    v.filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default()
}

/// `parseCandidates(text)`: the candidates in a model's reply, tidied (not
/// yet validated); empty when there is no JSON to read.
pub fn parse_candidates(text: &str) -> Vec<Candidate> {
    let s = strip_fences(text);
    let mut tries = Vec::new();
    if let (Some(a), Some(b)) = (s.find('{'), s.rfind('}')) {
        if b > a {
            tries.push(&s[a..=b]);
        }
    }
    if let (Some(c), Some(d)) = (s.find('['), s.rfind(']')) {
        if d > c {
            tries.push(&s[c..=d]);
        }
    }
    let mut list: Option<Vec<Value>> = None;
    for t in tries {
        let Ok(j) = serde_json::from_str::<Value>(t) else { continue };
        list = match &j {
            Value::Array(a) => Some(a.clone()),
            Value::Object(o) => match o.get("candidates") {
                Some(Value::Array(a)) => Some(a.clone()),
                _ if o.get("shape").is_some_and(js::truthy) => Some(vec![j.clone()]),
                _ => None,
            },
            _ => None,
        };
        if list.is_some() {
            break;
        }
    }
    let empty = Map::new();
    list.unwrap_or_default()
        .iter()
        .filter_map(|x| match x {
            Value::Object(o) => Some(o),
            Value::Array(_) => Some(&empty),
            _ => None,
        })
        .map(|x| {
            let outline = match x.get("outline") {
                Some(Value::String(o)) => squeeze_inner(js_trim(o)),
                _ => String::new(),
            };
            let mut shape = js_trim(&or_empty(x.get("shape"))).to_lowercase();
            if shape.is_empty() && !outline.is_empty() {
                shape = "custom".into();
            }
            let color = js_trim(&or_empty(x.get("color").filter(|v| js::truthy(v)).or(x.get("colour")))).to_uppercase();
            let layers = parse_layers(x.get("layers"));
            let parts = x.get("parts").and_then(Value::as_array).map(|ps| {
                ps.iter()
                    .filter_map(Value::as_object)
                    .map(|p| {
                        let mut o = Map::new();
                        o.insert("outline".into(), json!(match p.get("outline") {
                            Some(Value::String(d)) => squeeze_inner(js_trim(d)),
                            _ => String::new(),
                        }));
                        o.insert("color".into(), json!(js_trim(&or_empty(p.get("color").filter(|v| js::truthy(v)).or(p.get("colour")))).to_uppercase()));
                        if let Some(l) = parse_layers(p.get("layers")) {
                            o.insert("layers".into(), Value::Array(l));
                        }
                        if let Some(z) = p.get("z").filter(|v| v.is_number()) {
                            o.insert("z".into(), z.clone());
                        }
                        if let Some(d) = p.get("d").filter(|v| v.is_number()) {
                            o.insert("d".into(), d.clone());
                        }
                        Value::Object(o)
                    })
                    .collect::<Vec<_>>()
            });
            let eyes = parse_eyes(x.get("eyes"));
            let custom = shape == "custom";
            Candidate {
                name: js::slice_utf16(js_trim(&or_empty(x.get("name"))), 40),
                outline: custom.then_some(outline),
                layers: layers.filter(|_| custom),
                eyes: eyes.filter(|_| custom),
                parts: parts.filter(|_| custom),
                solids: x.get("solids").and_then(Value::as_array).filter(|_| custom).map(|s| s.iter().map(solid_of).collect()),
                shape,
                color,
                why: js::slice_utf16(js_trim(&or_empty(x.get("why"))), 240),
            }
        })
        .collect()
}

/// A reply's layers (`parseLayers`), tidied: objects `{path, fill (or
/// color), opacity?}` or the compact form `[path, fill, opacity?]`.
fn parse_layers(ls: Option<&Value>) -> Option<Vec<Value>> {
    ls?.as_array().map(|ls| {
        ls.iter()
            .filter_map(|l| match l {
                Value::Object(o) => Some(o.clone()),
                // the compact form the app uses: [path, fill, opacity?, "over"?]
                Value::Array(a) => {
                    let mut o = Map::new();
                    for (k, key) in ["path", "fill", "opacity"].iter().enumerate() {
                        if let Some(v) = a.get(k) {
                            o.insert((*key).into(), v.clone());
                        }
                    }
                    if a.get(3).and_then(Value::as_str) == Some("over") {
                        o.insert("over".into(), json!(true));
                    }
                    Some(o)
                }
                _ => None,
            })
            .map(|l| {
                let mut o = Map::new();
                o.insert("path".into(), json!(match l.get("path") {
                    Some(Value::String(p)) => squeeze_inner(js_trim(p)),
                    _ => String::new(),
                }));
                let fill = or_empty(l.get("fill").filter(|v| js::truthy(v)).or(l.get("color")));
                o.insert("fill".into(), json!(js_trim(&fill).to_uppercase()));
                if let Some(op) = l.get("opacity").filter(|v| v.is_number()) {
                    o.insert("opacity".into(), op.clone());
                }
                // drawn above the eyes (a fringe over one, an eyepatch, glasses): kept only when true
                if l.get("over") == Some(&Value::Bool(true)) {
                    o.insert("over".into(), json!(true));
                }
                Value::Object(o)
            })
            .collect::<Vec<_>>()
    })
}

/// A reply's eyes (`parseEyes`): a colour, or the placement object with
/// its known keys (colours upper-cased); anything else is none.
fn parse_eyes(e: Option<&Value>) -> Option<Value> {
    match e? {
        Value::String(c) => Some(json!(js_trim(c).to_uppercase())),
        Value::Object(o) => {
            let mut out = Map::new();
            for k in ["c", "at", "gap", "s", "rim", "rw", "shut"] {
                if let Some(v) = o.get(k) {
                    let v = match v {
                        Value::String(c) if k == "c" || k == "rim" => json!(js_trim(c).to_uppercase()),
                        Value::String(c) if k == "shut" => json!(js_trim(c).to_lowercase()),
                        v => v.clone(),
                    };
                    out.insert(k.into(), v);
                }
            }
            Some(Value::Object(out))
        }
        _ => None,
    }
}

/// `.replace(/```(?:json)?/gi, "")`
fn strip_fences(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("```") {
        out.push_str(&rest[..i]);
        rest = &rest[i + 3..];
        if rest.len() >= 4 && rest[..4].eq_ignore_ascii_case("json") {
            rest = &rest[4..];
        }
    }
    out.push_str(rest);
    out
}

/// `.replace(/\s+/g, " ")` of a trimmed string.
fn squeeze_inner(s: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in s.chars() {
        if js::is_space(c) {
            gap = true;
        } else {
            if gap {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        }
    }
    out
}

// The problems with one round of candidates as a set, and each one's.
fn check(cands: &[Candidate], image: bool, rich: bool) -> (Vec<Vec<String>>, Vec<String>) {
    let each: Vec<Vec<String>> = cands
        .iter()
        .map(|c| {
            let mut e = c.errors();
            if rich && !c.layers.as_ref().is_some_and(|l| !l.is_empty()) {
                e.push("a rich candidate needs layers and eyes".into());
            }
            e
        })
        .collect();
    let mut set = Vec::new();
    if cands.len() != 3 {
        set.push(format!("there must be exactly 3 candidates (got {})", cands.len()));
    }
    if rich {
        return (each, set);
    }
    if image && cands.first().map(|c| c.shape.as_str()) != Some("custom") {
        set.push("candidate 1 must be a custom outline tracing the image's subject".into());
    }
    if !image && !cands.is_empty() && !cands.iter().zip(&each).any(|(c, e)| is_kit(&c.shape) && e.is_empty()) {
        set.push("at least one candidate must be a kit shape".into());
    }
    (each, set)
}

/// `designCandidates({prompt, name, image}, {ask, pick})`: three
/// candidates. `ask(messages)` is the model call (Anthropic-shaped
/// messages) → the reply's text; `pick(prompt)` → a kit shape for when
/// nothing usable comes back. `image` is the user turn's image block.
///
/// With `self_check` (rich only), the set gets one look at itself drawn
/// flat ([`self_check_round`]); `preview(svgs)` → an image block per SVG,
/// or `None` to send the SVG text.
#[allow(clippy::too_many_arguments)]
pub async fn design_candidates<A, AF, K, KF, V, VF>(prompt: &str, name: &str, image: Option<Value>, rich: bool, self_check: bool, ask: A, pick: K, preview: V) -> Result<Vec<Candidate>, String>
where
    A: Fn(Vec<Value>) -> AF,
    AF: Future<Output = Result<String, String>>,
    K: Fn(String) -> KF,
    KF: Future<Output = String>,
    V: FnOnce(Vec<String>) -> VF,
    VF: Future<Output = Option<Vec<Value>>>,
{
    let has_image = image.is_some();
    let text = design_request(prompt, name, has_image, rich);
    let content = match image {
        Some(img) => json!([img, {"type": "text", "text": text}]),
        None => json!(text),
    };
    let mut messages = vec![json!({"role": "user", "content": content})];
    let first = messages[0].clone();
    let reply = ask(messages.clone()).await?;
    let mut cands = parse_candidates(&reply);
    let (each, set) = check(&cands, has_image, rich);
    let first_good: Vec<Candidate> = cands.iter().zip(&each).filter(|(_, e)| e.is_empty()).map(|(c, _)| c.clone()).collect();
    if !set.is_empty() || each.iter().any(|e| !e.is_empty()) {
        let mut lines = set;
        for (k, e) in each.iter().enumerate() {
            if !e.is_empty() {
                let c = &cands[k];
                let label = if !c.name.is_empty() {
                    c.name.as_str()
                } else if !c.shape.is_empty() {
                    c.shape.as_str()
                } else {
                    "unnamed"
                };
                lines.push(format!("candidate {} ({label}): {}", k + 1, e.join("; ")));
            }
        }
        if cands.is_empty() {
            lines.insert(0, "the reply had no JSON candidates".into());
        }
        let said = js_trim(&reply);
        messages.push(json!({"role": "assistant", "content": if said.is_empty() { "(empty)" } else { said }}));
        messages.push(json!({"role": "user", "content": format!("These problems need fixing:\n- {}\nReply with all 3 candidates again, in the same order, fixed. JSON only.", lines.join("\n- "))}));
        let reply = ask(messages).await?;
        cands = parse_candidates(&reply);
        let (each, _) = check(&cands, has_image, rich);
        cands = cands.into_iter().zip(each).filter(|(_, e)| e.is_empty()).map(|(c, _)| c).collect();
        // top up from the first round's good ones
        for c in first_good {
            if cands.len() >= 3 {
                break;
            }
            if !cands.iter().any(|x| x.shape == c.shape && x.color == c.color && x.outline == c.outline) {
                cands.push(c);
            }
        }
    }
    cands.truncate(3);
    if rich {
        if cands.is_empty() {
            return Err("no rich design came back that the engine can draw; try again or another model".into());
        }
        if self_check {
            return Ok(self_check_round(prompt, first, cands, &ask, preview).await);
        }
        return Ok(cands);
    }
    if !has_image && !cands.is_empty() && !cands.iter().any(|c| is_kit(&c.shape)) {
        let k = kit_fallback(&pick(prompt.to_owned()).await);
        cands.truncate(2);
        cands.push(k);
    }
    if cands.is_empty() {
        cands = vec![kit_fallback(&pick(prompt.to_owned()).await)];
    }
    Ok(cands)
}

/// `selfCheckRound`: each candidate drawn flat ([`preview_svg`]), shown to
/// the model once with the question whether it reads as the request; a
/// fix that passes the checks replaces its candidate, any other keeps it.
/// A failed call keeps the set as it is.
pub async fn self_check_round<A, AF, V, VF>(prompt: &str, first: Value, cands: Vec<Candidate>, ask: &A, preview: V) -> Vec<Candidate>
where
    A: Fn(Vec<Value>) -> AF,
    AF: Future<Output = Result<String, String>>,
    V: FnOnce(Vec<String>) -> VF,
    VF: Future<Output = Option<Vec<Value>>>,
{
    const COUNT: [&str; 4] = ["none", "one", "two", "three"];
    let svgs: Vec<String> = cands.iter().map(preview_svg).collect();
    let images = preview(svgs.clone()).await;
    let n = cands.len();
    let subject = if prompt.is_empty() { "the picture's subject".to_owned() } else { format!("the request \"{prompt}\"") };
    let mut content: Vec<Value> = Vec::new();
    for (k, c) in cands.iter().enumerate() {
        let label = if c.name.is_empty() { "unnamed" } else { &c.name };
        match &images {
            Some(imgs) => {
                content.push(json!({"type": "text", "text": format!("Design {} ({label}):", k + 1)}));
                content.push(imgs.get(k).cloned().unwrap_or(Value::Null));
            }
            None => content.push(json!({"type": "text", "text": format!("Design {} ({label}), as SVG:\n{}", k + 1, svgs[k])})),
        }
    }
    let count = COUNT.get(n).map(|s| s.to_string()).unwrap_or_else(|| n.to_string());
    content.push(json!({"type": "text", "text": format!("Here are your {count} designs drawn flat. For each, does it read as {subject}? Fix the outline, layers and eyes where it doesn't (missing hair volume, eyes off the lenses or band, features covering the eyes). Reply with all {n}, JSON only.")}));
    let said = js::stringify(&json!({"candidates": cands.iter().map(Candidate::json).collect::<Vec<_>>()}));
    let Ok(reply) = ask(vec![first, json!({"role": "assistant", "content": said}), json!({"role": "user", "content": content})]).await else { return cands };
    let fixed = parse_candidates(&reply);
    cands
        .into_iter()
        .enumerate()
        .map(|(k, c)| match fixed.get(k) {
            Some(f) if f.layers.as_ref().is_some_and(|l| !l.is_empty()) && f.errors().is_empty() => f.clone(),
            _ => c,
        })
        .collect()
}

// distance from p to the polygon's edge, positive inside
fn signed_distance(poly: &[P], [px, py]: P) -> f64 {
    let mut best = f64::INFINITY;
    for k in 0..poly.len() {
        let ([ax, ay], [bx, by]) = (poly[k], poly[(k + 1) % poly.len()]);
        let (ex, ey) = (bx - ax, by - ay);
        let len = ex * ex + ey * ey;
        let t = if len > 0.0 { (((px - ax) * ex + (py - ay) * ey) / len).min(1.0).max(0.0) } else { 0.0 };
        let (dx, dy) = (px - (ax + t * ex), py - (ay + t * ey));
        best = best.min((dx * dx + dy * dy).sqrt());
    }
    if inside(poly, px, py) {
        best
    } else {
        -best
    }
}

/// `inscribedCircle(poly)`: the largest circle inside the polygon, the
/// engine's grid search refined three times around the best cell.
pub fn inscribed_circle(poly: &[P]) -> (P, f64) {
    let f = |init: f64, g: fn(f64, f64) -> f64, k: usize| poly.iter().map(|p| p[k]).fold(init, g);
    let (x0, x1, y0, y1) = (f(f64::INFINITY, f64::min, 0), f(f64::NEG_INFINITY, f64::max, 0), f(f64::INFINITY, f64::min, 1), f(f64::NEG_INFINITY, f64::max, 1));
    let mut center = [(x0 + x1) / 2.0, (y0 + y1) / 2.0];
    let mut best = signed_distance(poly, center);
    let (mut hw, mut hh) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
    const GRID: f64 = 24.0;
    for _ in 0..4 {
        let c = center;
        for i in 0..=24 {
            for j in 0..=24 {
                let p = [c[0] - hw + (2.0 * hw * i as f64) / GRID, c[1] - hh + (2.0 * hh * j as f64) / GRID];
                let d = signed_distance(poly, p);
                if d > best {
                    best = d;
                    center = p;
                }
            }
        }
        hw *= 4.0 / GRID;
        hh *= 4.0 / GRID;
    }
    (center, best.max(0.0))
}

/// `eyePlacement(outline, eyes)`: where the preview draws the eyes, (at,
/// w, h, gap) in the 0..100 box: the recipe's `at`/`gap`/`s`, else the
/// engine's default in the upper part of the largest inscribed circle.
pub fn eye_placement(outline: &str, eyes: Option<&Value>) -> (P, f64, f64, f64) {
    let poly = flatten_path(outline).map(|f| f.points).unwrap_or_else(|_| vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]]);
    let (c, r) = inscribed_circle(&poly);
    let e = eyes.and_then(Value::as_object);
    let num = |k: &str| e.and_then(|e| e.get(k)).filter(|v| v.is_number()).and_then(Value::as_f64);
    let s = num("s").map(|s| s.max(EYE_SCALE.0).min(EYE_SCALE.1)).unwrap_or(1.0);
    let w = 0.24 * r * s;
    let at = match e.and_then(|e| e.get("at")).and_then(Value::as_array) {
        Some(a) => [a.first().and_then(Value::as_f64).unwrap_or(f64::NAN), a.get(1).and_then(Value::as_f64).unwrap_or(f64::NAN)],
        None => [c[0], c[1] - 0.15 * r],
    };
    (at, w, w * 2.2, num("gap").unwrap_or_else(|| (0.5 * r).max(w * 1.25)))
}

/// `String(Math.round(v * 100) / 100)`
fn n2(v: f64) -> String {
    js::number((v * 100.0 + 0.5).floor() / 100.0)
}

fn esc(v: &str) -> String {
    v.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `previewSvg(c)`: a rich candidate drawn flat, front view: the outline
/// in its shell colour, the layers back to front, two capsule eyes (with
/// their rims) where the engine would put them.
pub fn preview_svg(c: &Candidate) -> String {
    let mut out = String::from("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 100\" width=\"512\" height=\"512\"><rect width=\"100\" height=\"100\" fill=\"#3A3A3A\"/>");
    let outline = c.outline.clone().unwrap_or_default();
    let is_over = |l: &Value| l.get("over") == Some(&Value::Bool(true));
    let draw_layers = |out: &mut String, layers: Option<&Vec<Value>>, over: bool| {
        for l in layers.into_iter().flatten().filter(|l| is_over(l) == over) {
            let s = |k: &str| l.get(k).map(js::string_of).unwrap_or_else(|| "undefined".into());
            let op = l.get("opacity").filter(|v| v.is_number()).and_then(Value::as_f64).map(|o| format!(" opacity=\"{}\"", n2(o))).unwrap_or_default();
            out.push_str(&format!("<path d=\"{}\" fill=\"{}\"{op}/>", esc(&s("path")), esc(&s("fill"))));
        }
    };
    // a piece: its outline, then its layers below the eyes (over: false) or all of them
    let piece = |out: &mut String, outline: &str, color: &str, layers: Option<&Vec<Value>>, all: bool| {
        out.push_str(&format!("<path d=\"{}\" fill=\"{}\"/>", esc(outline), esc(color)));
        draw_layers(out, layers, false);
        if all {
            draw_layers(out, layers, true);
        }
    };
    // the parts in z order: behind the head (z < 0), then the head and its
    // eyes, then in front (z >= 0, the default)
    let z_of = |p: &Value| p.get("z").filter(|v| v.is_number()).and_then(Value::as_f64).unwrap_or(0.0);
    let mut parts: Vec<&Value> = c.parts.iter().flatten().collect();
    parts.sort_by(|a, b| z_of(a).partial_cmp(&z_of(b)).unwrap_or(std::cmp::Ordering::Equal));
    let draw_part = |out: &mut String, p: &Value, all: bool| {
        let s = |k: &str| p.get(k).map(js::string_of).unwrap_or_else(|| "undefined".into());
        piece(out, &s("outline"), &s("color"), p.get("layers").and_then(Value::as_array), all);
    };
    // behind the head, the head, the eyes, the over-layers of those, then the parts in front
    let back: Vec<&&Value> = parts.iter().filter(|p| z_of(p) < 0.0).collect();
    for p in &back {
        draw_part(&mut out, p, false);
    }
    piece(&mut out, &outline, &c.color, c.layers.as_ref(), false);
    let e = c.eyes.as_ref();
    let colour = match e {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(o)) => o.get("c").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_else(|| "#FFFFFF".into()),
        _ => "#FFFFFF".into(),
    };
    let (at, w, h, gap) = eye_placement(&outline, e);
    let capsule = |x: f64, y: f64, cw: f64, ch: f64, fill: &str| format!("<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" fill=\"{}\"/>", n2(x - cw / 2.0), n2(y - ch / 2.0), n2(cw), n2(ch), n2(cw / 2.0), esc(fill));
    let shut = e.and_then(|e| e.get("shut")).and_then(Value::as_str);
    let rim = e.and_then(Value::as_object).and_then(|o| Some((o.get("rim")?.as_str()?.to_owned(), o.get("rw").filter(|v| v.is_number())?.as_f64()?))).filter(|(_, rw)| *rw > 0.0);
    for side in [-1.0, 1.0] {
        let x = at[0] + (side * gap) / 2.0;
        if let Some((rc, rw)) = &rim {
            let t = rw * w;
            out.push_str(&capsule(x, at[1], w + 2.0 * t, h + 2.0 * t, rc));
        }
        if shut == Some(if side < 0.0 { "l" } else { "r" }) {
            // a closed eye: a short curved lid line
            out.push_str(&format!(
                "<path d=\"M{} {} Q{} {} {} {}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\" stroke-linecap=\"round\"/>",
                n2(x - w / 2.0), n2(at[1]), n2(x), n2(at[1] + h * 0.25), n2(x + w / 2.0), n2(at[1]), esc(&colour), n2(w * 0.35)
            ));
        } else {
            out.push_str(&capsule(x, at[1], w, h, &colour));
        }
    }
    for p in &back {
        draw_layers(&mut out, p.get("layers").and_then(Value::as_array), true);
    }
    draw_layers(&mut out, c.layers.as_ref(), true);
    for p in parts.iter().filter(|p| z_of(p) >= 0.0) {
        draw_part(&mut out, p, true);
    }
    out.push_str("</svg>");
    out
}

/// The kit picker's shape, in the primary ink: Design defaults to #FFFFFF.
fn kit_fallback(shape: &str) -> Candidate {
    let name = if is_kit(shape) {
        let mut c = shape.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    } else {
        "Blob".into()
    };
    Candidate { name, shape: shape.into(), color: "#FFFFFF".into(), outline: None, layers: None, eyes: None, parts: None, solids: None, why: "A kit shape picked for the description.".into() }
}

/// MOCK mode's outlines.
pub const MOCK_PEBBLE: &str = "M50 16 C73 16 88 31 88 52 C88 73 72 86 50 86 C28 86 12 73 12 52 C12 31 27 16 50 16 Z";
pub const MOCK_CAT: &str = "M18 44 L22 14 L42 28 Q50 26 58 28 L78 14 L82 44 C88 66 72 86 50 86 C28 86 12 66 18 44 Z";
pub const MOCK_RECEIPT: &str = "M26 14 L74 14 L74 80 L66 88 L58 80 L50 88 L42 80 L34 88 L26 80 Z";

/// `mockCandidates({image})`
pub fn mock_candidates(image: bool) -> Vec<Candidate> {
    let c = |name: &str, shape: &str, outline: Option<&str>, why: &str| Candidate { name: name.into(), shape: shape.into(), color: "#FFFFFF".into(), outline: outline.map(str::to_owned), layers: None, eyes: None, parts: None, solids: None, why: why.into() };
    let kit = c("Blob", "blob", None, "A friendly ball in the primary ink.");
    let cat = c("Cat Head", "custom", Some(MOCK_CAT), "A bold cat silhouette.");
    let pebble = c("Pebble", "custom", Some(MOCK_PEBBLE), "A calm, rounded pebble.");
    let receipt = c("Receipt", "custom", Some(MOCK_RECEIPT), "A receipt with a torn foot.");
    if image {
        vec![Candidate { name: "Image Subject".into(), why: "The image's subject as a silhouette.".into(), ..cat }, kit, pebble]
    } else {
        vec![kit, receipt, cat]
    }
}

/// `MOCK_RICH`: rich candidates, valid, hand-written.
pub fn mock_rich() -> Vec<Value> {
    vec![
        json!({
            "name": "Violet Ogre", "shape": "custom", "color": "#7A4FB5",
            "outline": "M50 12 C72 12 86 28 86 48 C90 66 76 88 50 90 C24 88 10 66 14 48 C14 28 28 12 50 12 Z",
            "layers": [
                {"path": "M20 40 C30 24 70 24 80 40 C66 34 34 34 20 40 Z", "fill": "#3A2A5C"},
                {"path": "M24 54 C24 42 76 42 76 54 C76 72 66 86 50 86 C34 86 24 72 24 54 Z", "fill": "#F5EBD6"},
                {"path": "M30 68 L38 68 L38 72 L30 72 Z", "fill": "#F2C3B5", "opacity": 0.6},
                {"path": "M62 68 L70 68 L70 72 L62 72 Z", "fill": "#F2C3B5", "opacity": 0.6},
            ],
            "eyes": "#FFC83D", "why": "A violet shell with a dark fringe and a cream face.",
        }),
        json!({
            "name": "Band Hero", "shape": "custom", "color": "#1E8E8A",
            "outline": "M50 12 C70 12 84 30 84 50 C84 72 70 88 50 88 C30 88 16 72 16 50 C16 30 30 12 50 12 Z",
            "layers": [{"path": "M20 40 L80 40 L80 54 L20 54 Z", "fill": "#F2C14E"}],
            "eyes": {"c": "#FFF6D8", "at": [50, 45], "gap": 24, "s": 1.1, "rim": "#16323A", "rw": 0.5},
            "why": "A teal head with a gold band; the eyes ringed in dark blue.",
        }),
        json!({
            "name": "Bucket Hat", "shape": "custom", "color": "#6B3A1E",
            "outline": "M22 54 C22 38 34 30 50 30 C66 30 78 38 78 54 C78 72 66 86 50 86 C34 86 22 72 22 54 Z",
            "layers": [{"path": "M28 56 C28 44 72 44 72 56 C72 72 62 82 50 82 C38 82 28 72 28 56 Z", "fill": "#F5D7B5"}],
            "eyes": {"c": "#1E1E1E", "at": [50, 58], "gap": 18},
            "parts": [{
                "outline": "M12 38 L28 34 L34 10 L66 10 L72 34 L88 38 L88 44 L12 44 Z",
                "color": "#4C8CC9",
                "layers": [{"path": "M32 24 L68 24 L68 30 L32 30 Z", "fill": "#F4F4F4"}],
                "z": 1,
            }],
            "why": "An auburn-haired head under a blue bucket hat with a white band.",
        }),
    ]
}

/// avatar-design.mjs `pictureRequest({prompt, image})`: the style, then
/// what the user typed on top of it; with no picture, the character is the
/// user's description.
pub fn picture_request(prompt: &str, image: bool) -> String {
    const LEAD: &str = "Turn the subject of the reference picture into a minimal flat \"Bot\" avatar. Use the picture only to recognise who the subject is; do not copy its pose, framing or background.";
    let style = match picture_style().strip_prefix(LEAD) {
        Some(rest) if !image => format!("Draw the character described below as a minimal flat \"Bot\" avatar.{rest}"),
        _ => picture_style().to_owned(),
    };
    let extra = js_trim(prompt);
    if extra.is_empty() {
        style
    } else {
        format!("{style}\n\n{}: {extra}", if image { "Also, from the user" } else { "The character" })
    }
}

/// The SVG Picture mode asks for (avatar-design.mjs `SVG_RULES`).
pub const SVG_RULES: &str = "Draw it as one SVG: viewBox=\"0 0 1024 1024\", full-bleed, the background covering every pixel. Flat shapes and simple linear or radial gradients only; no text, no <image>, no filters, no scripts. Reply with only the <svg>…</svg> markup.";

/// The SVG in a reply (`/<svg[\s\S]*<\/svg>/`), or why it can't be drawn.
pub fn picture_svg(reply: &str) -> Result<&str, String> {
    let svg = match (reply.find("<svg"), reply.rfind("</svg>")) {
        (Some(a), Some(b)) if b >= a => &reply[a..b + 6],
        _ => return Err("the model returned no SVG".into()),
    };
    // /<script|<image|<foreignObject|href\s*=\s*["']https?:/i
    let l = svg.to_lowercase();
    let external = l.match_indices("href").any(|(i, _)| {
        let rest = l[i + 4..].trim_start_matches(js::is_space);
        rest.strip_prefix('=').map(|r| r.trim_start_matches(js::is_space)).and_then(|r| r.strip_prefix('"').or_else(|| r.strip_prefix('\''))).is_some_and(|r| r.starts_with("http:") || r.starts_with("https:"))
    });
    if l.contains("<script") || l.contains("<image") || l.contains("<foreignobject") || external {
        return Err("the SVG used scripts or outside images".into());
    }
    Ok(svg)
}

#[cfg(test)]
mod tests;
