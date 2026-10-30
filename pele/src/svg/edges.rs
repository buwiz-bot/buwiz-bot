// Port of src/svg/edges.ts.

use super::builder::{esc_text, label_svg, num};
use super::ink::ink;
use crate::js;
use crate::layout::layered::ease;
use crate::text::label::Label;
use std::collections::HashMap;

pub const ARROW: f64 = 8.0;

pub struct EdgePath {
    pub d: String,
    pub tokens: Option<Vec<f64>>,
    pub sx: f64,
    pub sy: f64,
    pub sdx: f64,
    pub sdy: f64,
    pub ex: f64,
    pub ey: f64,
    pub edx: f64,
    pub edy: f64,
}

// The most a corner is rounded by in a stepped route.
const CORNER: f64 = 12.0;
// How far the handles of a wide turn reach along its two legs.
const HANDLE: f64 = 0.62;
// How far an edge runs straight before a marker at its end, where it has the room.
pub const LEAD: f64 = 6.0;

// How far from each side of each node the edges that meet it start to turn.
#[derive(Default)]
pub struct SideReach(HashMap<i64, f64>);

impl SideReach {
    pub fn add(&mut self, node: usize, side: i32, type_: &str) {
        let trim = marker_trim(type_);
        let key = node as i64 * 4 + side as i64;
        if side >= 0 && trim > 0.0 && trim + LEAD > self.0.get(&key).copied().unwrap_or(0.0) {
            self.0.insert(key, trim + LEAD);
        }
    }
    pub fn of(&self, node: usize, side: i32) -> f64 {
        if side >= 0 {
            self.0.get(&(node as i64 * 4 + side as i64)).copied().unwrap_or(0.0)
        } else {
            0.0
        }
    }
}

fn sign(v: f64) -> f64 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        v
    }
}

/// Builds the path for a route of x, y, axis triples, trimming both ends to leave room for markers.
pub fn route_path(route: &[f64], curve: Option<&str>, start_trim: f64, end_trim: f64, start_reach: Option<f64>, end_reach: Option<f64>) -> EdgePath {
    let start_reach = start_reach.unwrap_or(if start_trim > 0.0 { start_trim + LEAD } else { 0.0 });
    let end_reach = end_reach.unwrap_or(if end_trim > 0.0 { end_trim + LEAD } else { 0.0 });
    let linear = curve == Some("linear");
    let stepped = matches!(curve, Some("step" | "stepBefore" | "stepAfter"));
    let mut xs: Vec<f64> = Vec::new();
    let mut ys: Vec<f64> = Vec::new();
    let mut axes: Vec<f64> = Vec::new();
    let near = |ax: f64, ay: f64, bx: f64, by: f64| (ax - bx).abs() < 0.01 && (ay - by).abs() < 0.01;
    let mut i = 0;
    while i < route.len() {
        let x = route[i];
        let y = route[i + 1];
        let axis = route[i + 2];
        // A stepped route turns at right angles where a smooth one would curve.
        if stepped && !xs.is_empty() {
            let last = xs.len() - 1;
            if (x - xs[last]).abs() >= 0.01 && (y - ys[last]).abs() >= 0.01 {
                let px = xs[last];
                let py = ys[last];
                let my = if curve == Some("stepBefore") { py } else if curve == Some("stepAfter") { y } else { (py + y) / 2.0 };
                let mx = if curve == Some("stepBefore") { px } else if curve == Some("stepAfter") { x } else { (px + x) / 2.0 };
                let turns = if axis == 0.0 { [px, my, x, my] } else { [mx, py, mx, y] };
                let mut k = 0;
                while k < 4 {
                    if !(near(turns[k], turns[k + 1], px, py) || near(turns[k], turns[k + 1], x, y)) {
                        xs.push(turns[k]);
                        ys.push(turns[k + 1]);
                        axes.push(axis);
                    }
                    k += 2;
                }
            }
        }
        xs.push(x);
        ys.push(y);
        axes.push(axis);
        i += 3;
    }
    let mut last = xs.len() - 1;
    let aligned = |xs: &[f64], ys: &[f64], i: usize| (xs[i] - xs[i - 1]).abs() < 0.01 || (ys[i] - ys[i - 1]).abs() < 0.01;
    let straight = |xs: &[f64], ys: &[f64], i: usize| linear || aligned(xs, ys, i);
    // The way the route heads from point i to point j: along the flow where an S joins them.
    let unit = |xs: &[f64], ys: &[f64], i: usize, j: usize, axis: f64| -> (f64, f64) {
        if !straight(xs, ys, i.max(j)) {
            return if axis == 0.0 { (0.0, js::or(sign(ys[j] - ys[i]), || 1.0)) } else { (js::or(sign(xs[j] - xs[i]), || 1.0), 0.0) };
        }
        let dx = xs[j] - xs[i];
        let dy = ys[j] - ys[i];
        let len = js::or(dx.hypot(dy), || 1.0);
        (dx / len, dy / len)
    };
    let (sdx, sdy) = unit(&xs, &ys, 0, 1, axes[1]);
    let (edx, edy) = unit(&xs, &ys, last - 1, last, axes[last]);
    let sx = xs[0];
    let sy = ys[0];
    let ex = xs[last];
    let ey = ys[last];
    xs[0] += sdx * start_trim;
    ys[0] += sdy * start_trim;
    xs[last] -= edx * end_trim;
    ys[last] -= edy * end_trim;
    let length = |xs: &[f64], ys: &[f64], i: usize| (xs[i] - xs[i - 1]).abs() + (ys[i] - ys[i - 1]).abs();
    // Whether the route turns a corner at point i, from running one way to running the other.
    let turns = |xs: &[f64], ys: &[f64], i: usize, last: usize| {
        !linear
            && i > 0
            && i < last
            && aligned(xs, ys, i)
            && aligned(xs, ys, i + 1)
            && length(xs, ys, i) >= 0.01
            && length(xs, ys, i + 1) >= 0.01
            && (((xs[i] - xs[i - 1]).abs() < 0.01) != ((xs[i + 1] - xs[i]).abs() < 0.01))
    };
    let along = |xs: &[f64], ys: &[f64], i: usize, ux: f64, uy: f64| (xs[i] - xs[i - 1]).abs() * ux.abs() + (ys[i] - ys[i - 1]).abs() * uy.abs();
    let lead = |reach: f64, trim: f64, run: f64| {
        let room = (reach - LEAD - trim).max(0.0).min(run / 4.0);
        room + (reach - trim - room).min((run - room) / 4.0)
    };
    let arrives = if end_reach > end_trim && (!straight(&xs, &ys, last) || turns(&xs, &ys, last - 1, last)) { lead(end_reach, end_trim, along(&xs, &ys, last, edx, edy)) } else { 0.0 };
    let leaves = if start_reach > start_trim && (!straight(&xs, &ys, 1) || turns(&xs, &ys, 1, last)) { lead(start_reach, start_trim, along(&xs, &ys, 1, sdx, sdy)) } else { 0.0 };
    if arrives >= 1.0 {
        let (nx, ny, na) = (xs[last] - edx * arrives, ys[last] - edy * arrives, axes[last]);
        xs.insert(last, nx);
        ys.insert(last, ny);
        axes.insert(last, na);
        last += 1;
    }
    if leaves >= 1.0 {
        let (nx, ny, na) = (xs[0] + sdx * leaves, ys[0] + sdy * leaves, axes[1]);
        xs.insert(1, nx);
        ys.insert(1, ny);
        axes.insert(1, na);
        last += 1;
    }

    // Keep tokens to avoid parsing d when checking fills.
    let mut tokens: Vec<f64> = vec![77.0, 2.0];
    let mut d = String::new();
    let tok = |tokens: &mut Vec<f64>, v: f64| -> String {
        tokens.push(written(v));
        num(v)
    };
    d.push_str(&format!("M{},{}", tok(&mut tokens, xs[0]), tok(&mut tokens, ys[0])));
    let mut at_x = xs[0];
    let mut at_y = ys[0];
    let line_to = |d: &mut String, tokens: &mut Vec<f64>, at_x: &mut f64, at_y: &mut f64, x: f64, y: f64| {
        if near(x, y, *at_x, *at_y) {
            return;
        }
        tokens.push(76.0);
        tokens.push(2.0);
        let a = tok(tokens, x);
        let b = tok(tokens, y);
        d.push_str(&format!("L{},{}", a, b));
        *at_x = x;
        *at_y = y;
    };
    for i in 1..=last {
        let x = xs[i];
        let y = ys[i];
        if !straight(&xs, &ys, i) {
            let down = axes[i] == 0.0;
            let along = if down { y - at_y } else { x - at_x };
            let k = along.abs() * ease(if down { x - at_x } else { y - at_y }, along);
            let ux = if down { 0.0 } else { sign(along) * k };
            let uy = if down { sign(along) * k } else { 0.0 };
            tokens.push(67.0);
            tokens.push(6.0);
            let p = [tok(&mut tokens, at_x + ux), tok(&mut tokens, at_y + uy), tok(&mut tokens, x - ux), tok(&mut tokens, y - uy), tok(&mut tokens, x), tok(&mut tokens, y)];
            d.push_str(&format!("C{},{} {},{} {},{}", p[0], p[1], p[2], p[3], p[4], p[5]));
            at_x = x;
            at_y = y;
            continue;
        }
        if !turns(&xs, &ys, i, last) {
            // A point part of the way along one straight run adds nothing to the path.
            if i < last && aligned(&xs, &ys, i) && aligned(&xs, &ys, i + 1) && (x - xs[i - 1]) * (xs[i + 1] - x) + (y - ys[i - 1]) * (ys[i + 1] - y) > 0.0 {
                continue;
            }
            line_to(&mut d, &mut tokens, &mut at_x, &mut at_y, x, y);
            continue;
        }
        let mut before = if turns(&xs, &ys, i - 1, last) { length(&xs, &ys, i) / 2.0 } else { length(&xs, &ys, i) };
        let mut after = if turns(&xs, &ys, i + 1, last) { length(&xs, &ys, i + 1) / 2.0 } else { length(&xs, &ys, i + 1) };
        if stepped {
            let m = CORNER.min(before).min(after);
            before = m;
            after = m;
        }
        let (ux, uy) = unit(&xs, &ys, i - 1, i, axes[i]);
        let (vx, vy) = unit(&xs, &ys, i, i + 1, axes[i + 1]);
        line_to(&mut d, &mut tokens, &mut at_x, &mut at_y, x - ux * before, y - uy * before);
        at_x = x + vx * after;
        at_y = y + vy * after;
        if stepped {
            tokens.push(81.0);
            tokens.push(4.0);
            let p = [tok(&mut tokens, x), tok(&mut tokens, y), tok(&mut tokens, at_x), tok(&mut tokens, at_y)];
            d.push_str(&format!("Q{},{} {},{}", p[0], p[1], p[2], p[3]));
        } else {
            tokens.push(67.0);
            tokens.push(6.0);
            let p = [
                tok(&mut tokens, x - ux * before * (1.0 - HANDLE)),
                tok(&mut tokens, y - uy * before * (1.0 - HANDLE)),
                tok(&mut tokens, x + vx * after * (1.0 - HANDLE)),
                tok(&mut tokens, y + vy * after * (1.0 - HANDLE)),
                tok(&mut tokens, at_x),
                tok(&mut tokens, at_y),
            ];
            d.push_str(&format!("C{},{} {},{} {},{}", p[0], p[1], p[2], p[3], p[4], p[5]));
        }
    }
    EdgePath { d, tokens: Some(tokens), sx, sy, sdx: -sdx, sdy: -sdy, ex, ey, edx, edy }
}

pub fn marker_trim(type_: &str) -> f64 {
    match type_ {
        "arrow_point" => ARROW - 1.0,
        "arrow_circle" => ARROW,
        "arrow_cross" => 4.0,
        _ => 0.0,
    }
}

// Draws a marker whose tip is at (x, y), pointing along (dx, dy).
pub fn marker(type_: &str, x: f64, y: f64, dx: f64, dy: f64, color: &str, stroke: &str) -> String {
    let nx = -dy;
    let ny = dx;
    match type_ {
        "arrow_point" => {
            let bx = x - dx * ARROW;
            let by = y - dy * ARROW;
            let half = ARROW * 0.42;
            format!(
                "<path class=\"pele-marker\" d=\"M{},{}L{},{}L{},{}Z\" fill=\"{}\" stroke=\"{}\"/>",
                num(x),
                num(y),
                num(bx + nx * half),
                num(by + ny * half),
                num(bx - nx * half),
                num(by - ny * half),
                color,
                stroke
            )
        }
        "arrow_circle" => format!("<circle class=\"pele-marker\" cx=\"{}\" cy=\"{}\" r=\"3.5\" fill=\"{}\" stroke=\"{}\"/>", num(x - dx * 4.0), num(y - dy * 4.0), color, stroke),
        "arrow_cross" => {
            let cx = x - dx * 4.0;
            let cy = y - dy * 4.0;
            let a = 3.5;
            let ux = (dx + nx) * a;
            let uy = (dy + ny) * a;
            let vx = (dx - nx) * a;
            let vy = (dy - ny) * a;
            format!("<path class=\"pele-marker\" d=\"M{},{}L{},{}M{},{}L{},{}\"/>", num(cx - ux), num(cy - uy), num(cx + ux), num(cy + uy), num(cx - vx), num(cy - vy), num(cx + vx), num(cy + vy))
        }
        _ => String::new(),
    }
}

// Match the value parsed from num() output.
fn written(v: f64) -> f64 {
    let n = js::round(v * 100.0);
    if n == 0.0 {
        0.0
    } else {
        n / 100.0
    }
}

// Each command stores its character code, argument count, and arguments.
fn tokenize(d: &str) -> Vec<f64> {
    const TENS: [f64; 23] = [1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16, 1e17, 1e18, 1e19, 1e20, 1e21, 1e22];
    let b = d.as_bytes();
    let at_ = |j: usize| -> i32 { b.get(j).map_or(-1, |&c| c as i32) };
    let mut out: Vec<f64> = Vec::new();
    let mut at: isize = -1;
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as i32;
        if (65..=90).contains(&c) && c != 69 {
            at = out.len() as isize + 1;
            out.push(c as f64);
            out.push(0.0);
            i += 1;
        } else if c == 45 || c == 46 || (48..=57).contains(&c) {
            // Scale exact integers to match Number() rounding; otherwise use Number().
            let mut j = if c == 45 { i + 1 } else { i };
            let mut m: f64 = 0.0;
            let mut places: i32 = -1;
            loop {
                let k = at_(j);
                if !((48..=57).contains(&k) || (k == 46 && places < 0)) {
                    break;
                }
                if k == 46 {
                    places = 0;
                } else {
                    m = m * 10.0 + (k - 48) as f64;
                    if places >= 0 {
                        places += 1;
                    }
                }
                j += 1;
            }
            let v: f64;
            if (at_(j) | 32) == 101 || m > 9007199254740991.0 || places > 22 {
                loop {
                    let k = at_(j);
                    let ok = k == 46 || k == 101 || k == 69 || (48..=57).contains(&k) || ((k == 45 || k == 43) && j > 0 && (at_(j - 1) | 32) == 101);
                    if !ok {
                        break;
                    }
                    j += 1;
                }
                v = js::to_number(&d[i..j]);
            } else {
                let mut x = if places > 0 { m / TENS[places as usize] } else { m };
                if c == 45 {
                    x = -x;
                }
                v = x;
            }
            if at >= 0 {
                out.push(v);
                out[at as usize] += 1.0;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

fn sample(tokens: &[f64]) -> Vec<f64> {
    let mut out = Vec::new();
    let mut x = 0.0;
    let mut y = 0.0;
    let mut i = 0;
    while i < tokens.len() {
        let cmd = tokens[i] as i32;
        let count = tokens[i + 1] as usize;
        let s = i + 2;
        let e = s + count;
        i = e;
        if cmd == 72 || cmd == 86 {
            for k in s..e {
                if cmd == 72 {
                    x = tokens[k];
                } else {
                    y = tokens[k];
                }
                out.push(x);
                out.push(y);
            }
        } else if cmd == 67 || cmd == 81 {
            let cubic = cmd == 67;
            let step = if cubic { 6 } else { 4 };
            let mut k = s;
            while k + step <= e {
                let (x1, y1, x2, y2) = (tokens[k], tokens[k + 1], tokens[k + 2], tokens[k + 3]);
                let x3 = if cubic { tokens[k + 4] } else { x2 };
                let y3 = if cubic { tokens[k + 5] } else { y2 };
                let mut t = 0.125;
                while t <= 1.0 {
                    // Preserve the original De Casteljau operation order to keep rounding unchanged.
                    let mut ax = x + (x1 - x) * t;
                    let mut ay = y + (y1 - y) * t;
                    let mut bx = x1 + (x2 - x1) * t;
                    let mut by = y1 + (y2 - y1) * t;
                    if cubic {
                        let cx = x2 + (x3 - x2) * t;
                        let cy = y2 + (y3 - y2) * t;
                        ax += (bx - ax) * t;
                        ay += (by - ay) * t;
                        bx += (cx - bx) * t;
                        by += (cy - by) * t;
                    }
                    out.push(ax + (bx - ax) * t);
                    out.push(ay + (by - ay) * t);
                    t += 0.125;
                }
                x = x3;
                y = y3;
                k += step;
            }
        } else if cmd == 65 {
            let mut k = s;
            while k + 7 <= e {
                arc(&mut out, x, y, tokens[k], tokens[k + 1], tokens[k + 3], tokens[k + 4], tokens[k + 5], tokens[k + 6]);
                x = tokens[k + 5];
                y = tokens[k + 6];
                k += 7;
            }
        } else {
            let mut k = s;
            while k + 2 <= e {
                if cmd == 77 && k == s && !out.is_empty() {
                    out.push(f64::NAN);
                    out.push(f64::NAN);
                }
                x = tokens[k];
                y = tokens[k + 1];
                out.push(x);
                out.push(y);
                k += 2;
            }
        }
    }
    out
}

// Samples an unrotated arc from (x1, y1), after SVG's conversion from end points to a center.
#[allow(clippy::too_many_arguments)]
fn arc(out: &mut Vec<f64>, x1: f64, y1: f64, mut rx: f64, mut ry: f64, large: f64, sweep: f64, x2: f64, y2: f64) {
    let hx = (x1 - x2) / 2.0;
    let hy = (y1 - y2) / 2.0;
    if rx == 0.0 || ry == 0.0 || (hx == 0.0 && hy == 0.0) {
        out.push(x2);
        out.push(y2);
        return;
    }
    let scale = 1f64.max(((hx * hx) / (rx * rx) + (hy * hy) / (ry * ry)).sqrt());
    rx *= scale;
    ry *= scale;
    let k = (if large == sweep { -1.0 } else { 1.0 }) * (0f64.max((rx * rx * ry * ry - rx * rx * hy * hy - ry * ry * hx * hx) / (rx * rx * hy * hy + ry * ry * hx * hx))).sqrt();
    let cx = (k * rx * hy) / ry + (x1 + x2) / 2.0;
    let cy = (-k * ry * hx) / rx + (y1 + y2) / 2.0;
    let from = ((y1 - cy) / ry).atan2((x1 - cx) / rx);
    let mut turn = ((y2 - cy) / ry).atan2((x2 - cx) / rx) - from;
    if sweep != 0.0 && turn < 0.0 {
        turn += 2.0 * std::f64::consts::PI;
    } else if sweep == 0.0 && turn > 0.0 {
        turn -= 2.0 * std::f64::consts::PI;
    }
    for i in 1..=8 {
        out.push(cx + rx * (from + (turn * i as f64) / 8.0).cos());
        out.push(cy + ry * (from + (turn * i as f64) / 8.0).sin());
    }
}

// Test segment overlap with the box using Liang-Barsky clipping.
#[allow(clippy::too_many_arguments)]
fn clips(x: f64, y: f64, dx: f64, dy: f64, l: f64, t: f64, r: f64, b: f64) -> bool {
    let mut lo = 0.0;
    let mut hi = 1.0;
    for side in 0..4 {
        let p = match side {
            0 => -dx,
            1 => dx,
            2 => -dy,
            _ => dy,
        };
        let q = match side {
            0 => x - l,
            1 => r - x,
            2 => y - t,
            _ => b - y,
        };
        if p == 0.0 {
            if q < 0.0 {
                hi = -1.0;
            }
        } else if p < 0.0 {
            lo = f64::max(lo, q / p);
        } else {
            hi = f64::min(hi, q / p);
        }
    }
    lo < hi
}

fn enters(points: &[f64], l: f64, t: f64, r: f64, b: f64) -> bool {
    let mut k = 0;
    while k + 2 < points.len() {
        let (x, y, x2, y2) = (points[k], points[k + 1], points[k + 2], points[k + 3]);
        k += 2;
        if (x < l && x2 < l) || (x > r && x2 > r) || (y < t && y2 < t) || (y > b && y2 > b) {
            continue;
        }
        if clips(x, y, x2 - x, y2 - y, l, t, r, b) {
            return true;
        }
    }
    false
}

// Fill bounds are [left, top, right, bottom].
pub struct Fill {
    pub bx: [f64; 4],
    pub shape: String,
}

pub enum Under<'a> {
    Off,
    Crosses,
    On(&'a Fill),
}

// The innermost fill holding the whole edge and its marker tips.
pub fn fill_under<'a>(d: &str, tips: &[f64], fills: &'a [Fill], tokens: Option<&[f64]>) -> Under<'a> {
    if fills.is_empty() {
        return Under::Off;
    }
    let points = match tokens {
        Some(t) => sample(t),
        None => sample(&tokenize(d)),
    };
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let mut k = 0;
    while k < points.len() + tips.len() {
        let x = if k < points.len() { points[k] } else { tips[k - points.len()] };
        let y = if k < points.len() { points[k + 1] } else { tips[k + 1 - points.len()] };
        if x < min_x {
            min_x = x;
        }
        if x > max_x {
            max_x = x;
        }
        if y < min_y {
            min_y = y;
        }
        if y > max_y {
            max_y = y;
        }
        k += 2;
    }
    let mut holder: Option<&Fill> = None;
    let mut least = f64::INFINITY;
    for fill in fills {
        let [l, t, r, b] = fill.bx;
        if max_x < l + 0.5 || min_x > r - 0.5 || max_y < t + 0.5 || min_y > b - 0.5 {
            continue;
        }
        let mut touched = enters(&points, l + 0.5, t + 0.5, r - 0.5, b - 0.5);
        let mut k = 0;
        while k < tips.len() && !touched {
            touched = tips[k] > l + 0.5 && tips[k] < r - 0.5 && tips[k + 1] > t + 0.5 && tips[k + 1] < b - 0.5;
            k += 2;
        }
        if !touched {
            continue;
        }
        if min_x < l || max_x > r || min_y < t || max_y > b {
            return Under::Crosses;
        }
        let area = (r - l) * (b - t);
        if area < least {
            least = area;
            holder = Some(fill);
        }
    }
    match holder {
        Some(f) => Under::On(f),
        None => Under::Off,
    }
}

// The fill's ink for an edge on it, or '' for a casing when unknown or when the line style sets a stroke.
pub fn edge_ink(fill: &Fill, line: &str) -> String {
    if line.contains("\"stroke:") || line.contains(";stroke:") {
        return String::new();
    }
    ink(&fill.shape, "").unwrap_or("").to_string()
}

/// The ink of the innermost fill that holds the whole edge, Some("") for a casing where it
/// crosses a border, or None off fills.
pub fn over_fills(d: &str, tips: &[f64], fills: &[Fill], line: &str, tokens: Option<&[f64]>) -> Option<String> {
    match fill_under(d, tips, fills, tokens) {
        Under::Off => None,
        Under::Crosses => Some(String::new()),
        Under::On(fill) => Some(edge_ink(fill, line)),
    }
}

const CASE: f64 = 4.0;

/// `new RegExp(`[";]${prop}:([^;"]+);`).exec(line)?.[1].trim()`
fn decl(line: &str, prop: &str) -> Option<String> {
    if !line.contains(prop) {
        return None;
    }
    let b = line.as_bytes();
    let pat = format!("{}:", prop);
    let mut from = 0;
    while let Some(p) = line[from..].find(&pat) {
        let at = from + p;
        if at > 0 && (b[at - 1] == b'"' || b[at - 1] == b';') {
            let s = at + pat.len();
            let e = line[s..].find([';', '"']).map_or(line.len(), |q| q + s);
            if e > s && b.get(e) == Some(&b';') {
                return Some(js::trim(&line[s..e]).to_string());
            }
        }
        from = at + 1;
    }
    None
}

// A wider page-colored stroke under an edge, as faint as it; markers drop their class so marker styles skip it.
pub fn casing(d: &str, markers: &str, line: &str, thick: bool) -> String {
    let stroke = decl(line, "stroke");
    if stroke.as_deref() == Some("none") || stroke.as_deref() == Some("transparent") {
        return String::new();
    }
    let width = if thick { Some("2.5".to_string()) } else { decl(line, "stroke-width") };
    let mut attrs = String::new();
    let mut style = String::new();
    if let Some(w) = &width {
        let body = w.strip_suffix("px").unwrap_or(w);
        if !body.is_empty() && body.bytes().all(|c| c.is_ascii_digit() || c == b'.') {
            if js::parse_float(w) != 1.0 {
                attrs = format!(" stroke-width=\"{}\"", num(js::parse_float(w) + CASE));
            }
        } else {
            style.push_str(&format!("stroke-width:calc({} + {}px);", w, js::num_str(CASE)));
        }
    }
    for prop in ["opacity", "stroke-opacity"] {
        if let Some(value) = decl(line, prop) {
            style.push_str(&format!("{}:{};", prop, value));
        }
    }
    let body = format!("<path d=\"{}\"/>", d) + &markers.replace(" class=\"pele-marker\"", "");
    if !attrs.is_empty() || !style.is_empty() {
        format!("<g{}{}>{}</g>", attrs, if style.is_empty() { String::new() } else { format!(" style=\"{}\"", style) }, body)
    } else {
        body
    }
}

pub fn edge_casing(path: &EdgePath, start_type: &str, end_type: &str, line: &str, thick: bool) -> String {
    casing(
        &path.d,
        &(marker(start_type, path.sx, path.sy, path.sdx, path.sdy, "var(--_bg)", "var(--_bg)") + &marker(end_type, path.ex, path.ey, path.edx, path.edy, "var(--_bg)", "var(--_bg)")),
        line,
        thick,
    )
}

pub fn casings_svg(casings: &str) -> String {
    if casings.is_empty() {
        return String::new();
    }
    format!(
        "<g class=\"pele-casings\" fill=\"none\" stroke=\"var(--_bg)\" stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\" pointer-events=\"none\">{}</g>",
        js::num_str(1.0 + CASE),
        casings
    )
}

// An edge label on a patch of the background, so the line does not run through the text.
pub fn edge_label_svg(id: &str, label: &Label, x: f64, y: f64, attrs: &str) -> String {
    let w = label.width + 8.0;
    let h = label.height;
    format!(
        "<g class=\"pele-edge-label\" data-id=\"{}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"3\" fill=\"var(--_bg)\"/>{}</g>",
        esc_text(id),
        num(x - w / 2.0),
        num(y - h / 2.0),
        num(w),
        num(h),
        label_svg(label, x, y, attrs)
    )
}

pub struct LoopLabel {
    pub label_w: f64,
    pub label_h: f64,
    pub label_x: f64,
    pub label_y: f64,
}

// A self-loop beside a node of drawn size w by h: out of the side that lies across the flow and
// back in. `k` counts the loops already on the node. Also places the edge's label beyond the loop.
#[allow(clippy::too_many_arguments)]
pub fn loop_path(cx: f64, cy: f64, w: f64, h: f64, k: usize, sideways: bool, base: f64, trim: f64, e: &mut LoopLabel) -> EdgePath {
    let half = (if sideways { w } else { h }) / 2.0;
    let k = k as f64;
    let spread = (half - 4.0).min(8.0 + k * 6.0);
    let reach = base + k * 8.0;
    let len = js::or(reach.hypot(spread), || 1.0);
    // Local frame: `out` points away from the node, `along` runs along its side.
    let at = |out: f64, along: f64| {
        if sideways {
            format!("{},{}", num(cx + along), num(cy + h / 2.0 + out))
        } else {
            format!("{},{}", num(cx + w / 2.0 + out), num(cy + along))
        }
    };
    let tx = (reach / len) * trim;
    let ty = (spread / len) * trim;
    if sideways {
        e.label_x = cx;
        e.label_y = cy + h / 2.0 + reach + 4.0 + e.label_h / 2.0;
    } else {
        e.label_x = cx + w / 2.0 + reach + 4.0 + e.label_w / 2.0;
        e.label_y = cy;
    }
    EdgePath {
        d: format!("M{}C{} {} {}", at(0.0, -spread), at(reach, -spread * 2.0), at(reach, spread * 2.0), at(tx, spread + ty)),
        tokens: None,
        sx: if sideways { cx - spread } else { cx + w / 2.0 },
        sy: if sideways { cy + h / 2.0 } else { cy - spread },
        sdx: if sideways { 0.0 } else { -1.0 },
        sdy: if sideways { -1.0 } else { 0.0 },
        ex: if sideways { cx + spread } else { cx + w / 2.0 },
        ey: if sideways { cy + h / 2.0 } else { cy + spread },
        edx: if sideways { -spread / len } else { -reach / len },
        edy: if sideways { -reach / len } else { -spread / len },
    }
}
