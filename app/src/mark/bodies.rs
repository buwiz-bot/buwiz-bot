//! Body geometry — the seats, eye loops and outlines `Uh`'s raster path
//! needs (`Yv`/`Cs`, `cv`, `gS`/`xh`/`yS`/`SS`/`MS`/`TS`/`ES`, the outline
//! builders behind `Zr`/`fh`/`Wv`, the eye loop generators `lv`/`uv`, and
//! the `markatlas` reader that answers `IM`'s body outline exactly as
//! blob.tsx's `Wn.get` shim does).
//!
//! Dropped from the source: every body's `silhouette` field and the live
//! silhouette solve (`bh`/`kS`/`wS`/`xS`/`ki`/`bS`/`_M`/`GS`, plus `Jn`,
//! `ha`/`yh`, `rS`/`oS`, `Jv`/`eS`, `OS`/`LS`/`NS`/`FS`/`DS`/`zS`, `td`,
//! `jS`, `Vl`) — the atlas covers every reachable body, exactly like the
//! shell's shim (a body with no atlas draws nothing, where the JS would
//! re-solve live; no picker shape or clip reaches such a body).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::atlas_data::{AtlasEntry, ATLAS, ATLAS_K, ATLAS_STEP};
use super::clips::HEART_OUTLINE;
use super::{Eye, V2, V3};

/// `x` — every body path lives in a 228.5px box centered here.
pub(crate) const X: f64 = 114.2705;
const TAU: f64 = std::f64::consts::PI * 2.0;
const BV: f64 = 228.44;

fn te(e: f64) -> f64 {
    (e * 100.0).round() / 100.0
}
fn dc(e: f64, t: f64, n: f64) -> f64 {
    if e < t {
        t
    } else if e > n {
        n
    } else {
        e
    }
}

/// `un`/`qt` — the path-number formatter.
fn un(e: f64) -> String {
    format!("{}", (e * 100.0).round() / 100.0)
}

// ---------------------------------------------------------------------------
// `yn` — the path string builder (M/L/C/Q/arc, all rounded to 0.01px)
// ---------------------------------------------------------------------------

struct PathB {
    d: String,
}

impl PathB {
    fn new() -> PathB {
        PathB { d: String::new() }
    }
    fn mv(&mut self, t: f64, n: f64) {
        self.d += &format!("M{} {}", te(t), te(n));
    }
    fn line(&mut self, t: f64, n: f64) {
        self.d += &format!("L{} {}", te(t), te(n));
    }
    fn curve(&mut self, t: f64, n: f64, r: f64, o: f64, s: f64, i: f64) {
        self.d += &format!(
            "C{} {} {} {} {} {}",
            te(t),
            te(n),
            te(r),
            te(o),
            te(s),
            te(i)
        );
    }
    /// `corner(prev, pt, next, radius)` — rounded corner at `pt`.
    fn corner(&mut self, t: V2, n: V2, r: V2, o: f64) {
        let s = |l: V2, d: V2| -> V2 {
            let f = l[0] - d[0];
            let h = l[1] - d[1];
            let p = (f * f + h * h).sqrt();
            let p = if p == 0.0 { 1.0 } else { p };
            [(l[0] - d[0]) / p, (l[1] - d[1]) / p]
        };
        let i = s(t, n);
        let a = s(r, n);
        let c = [n[0] + i[0] * o, n[1] + i[1] * o];
        let u = [n[0] + a[0] * o, n[1] + a[1] * o];
        if self.d.is_empty() {
            self.mv(c[0], c[1]);
        } else {
            self.line(c[0], c[1]);
        }
        self.d += &format!("Q{} {} {} {}", te(n[0]), te(n[1]), te(u[0]), te(u[1]));
    }
    /// `arc(cx, cy, rx, ry, start, end)` — cubic-approximated arc.
    fn arc(&mut self, t: f64, n: f64, r: f64, o: f64, s: f64, i: f64) {
        let a = ((i - s).abs() / (std::f64::consts::PI / 2.0))
            .ceil()
            .max(1.0) as usize;
        let c = (i - s) / a as f64;
        let u = 4.0 / 3.0 * (c / 4.0).tan();
        let mut l = s;
        for _ in 0..a {
            let f = l + c;
            let h = [t + r * l.cos(), n + o * l.sin()];
            let p = [t + r * f.cos(), n + o * f.sin()];
            self.curve(
                h[0] - u * r * l.sin(),
                h[1] + u * o * l.cos(),
                p[0] + u * r * f.sin(),
                p[1] - u * o * f.cos(),
                p[0],
                p[1],
            );
            l = f;
        }
    }
    fn close(mut self) -> String {
        self.d += "Z";
        self.d
    }
}

// ---------------------------------------------------------------------------
// outline builders (`xv`/`ur`/`fc`/`lh`/`dh`/`Cl`/`vv`/`Sv`/`Mv`/`Tv`/`Ev`/
// `Pv`/`Av`/`Iv`/`_v`/`Cv`/`Lv`/`Nv`/`Fv`/`Dv`, `us`/`zv`/`jv`/`qv`/`Hv`/`Vv`/
// `hc`/`fh`/`Wv`/`Gv`/`Zr`/`mt`/`Ol`/`gh`/`pa`/`kh`/`fa`/`Jt`)
// ---------------------------------------------------------------------------

/// `xv` — closed Catmull-Rom spline through `e`.
fn xv(e: &[V2]) -> String {
    let t = e.len();
    let mut n = format!("M{} {}", te(e[0][0]), te(e[0][1]));
    for r in 0..t {
        let o = e[(r + t - 1) % t];
        let s = e[r];
        let i = e[(r + 1) % t];
        let a = e[(r + 2) % t];
        n += &format!(
            "C{} {} {} {} {} {}",
            te(s[0] + (i[0] - o[0]) / 6.0),
            te(s[1] + (i[1] - o[1]) / 6.0),
            te(i[0] - (a[0] - s[0]) / 6.0),
            te(i[1] - (a[1] - s[1]) / 6.0),
            te(i[0]),
            te(i[1])
        );
    }
    n + "Z"
}

/// `ur` — a parametric outline sampled into a smooth path.
fn ur(e: impl Fn(f64) -> V2, t: usize) -> String {
    let n: Vec<V2> = (0..t).map(|r| e(r as f64 / t as f64 * TAU)).collect();
    xv(&n)
}

/// `fc` — a polygon path with rounded corners (`t` is one radius or per-corner).
fn fc(e: &[V2], t: &[f64]) -> String {
    let mut n = PathB::new();
    let r = e.len();
    for o in 0..r {
        let s = t[o % t.len()];
        n.corner(e[(o + r - 1) % r], e[o], e[(o + 1) % r], s);
    }
    n.close()
}

/// `lh` — a regular polygon path.
fn lh(e: f64, t: usize, n: &[f64], r: f64) -> String {
    fc(
        &(0..t)
            .map(|s| {
                let i = r + s as f64 / t as f64 * TAU;
                [X + i.cos() * e, X + i.sin() * e]
            })
            .collect::<Vec<_>>(),
        n,
    )
}

/// `dh` — the outer hull of several discs (the cloud).
fn dh(e: &[[f64; 3]], t: usize) -> String {
    ur(
        |n| {
            let r = n.cos();
            let o = n.sin();
            let mut s = 0.0f64;
            for &[i, a, c] in e {
                let u = i - X;
                let l = a - X;
                let d = r * u + o * l;
                let f = d * d - (u * u + l * l) + c * c;
                if f <= 0.0 {
                    continue;
                }
                let h = d + f.sqrt();
                if h > s {
                    s = h;
                }
            }
            [X + r * s, X + o * s]
        },
        t,
    )
}

/// `Cl` — a superellipse `|x|^n + |y|^n = 1` scaled to `e`×`t`.
fn cl_path(e: f64, t: f64, n: f64) -> String {
    ur(
        |r| {
            let o = r.cos();
            let s = r.sin();
            [
                X + o.signum() * o.abs().powf(2.0 / n) * e,
                X + s.signum() * s.abs().powf(2.0 / n) * t,
            ]
        },
        160,
    )
}

/// `vv` — a tablet (stadium sideways).
fn vv(e: f64, t: f64) -> String {
    let mut p = PathB::new();
    p.mv(X - e + t, X - t);
    p.line(X + e - t, X - t);
    p.arc(X + e - t, X, t, t, -std::f64::consts::PI / 2.0, std::f64::consts::PI / 2.0);
    p.line(X - e + t, X + t);
    p.arc(X - e + t, X, t, t, std::f64::consts::PI / 2.0, std::f64::consts::PI * 3.0 / 2.0);
    p.close()
}

/// `Sv` — a capsule (stadium upright).
fn sv(e: f64, t: f64) -> String {
    let mut p = PathB::new();
    p.mv(X - e, X + t - e);
    p.line(X - e, X - t + e);
    p.arc(X, X - t + e, e, e, std::f64::consts::PI, TAU);
    p.line(X + e, X + t - e);
    p.arc(X, X + t - e, e, e, 0.0, std::f64::consts::PI);
    p.close()
}

/// `Mv` — a cylinder (flat-ish sides, curved caps).
fn mv_path(e: f64, t: f64, n: f64) -> String {
    let mut p = PathB::new();
    p.mv(X - e, X - t + n);
    p.arc(X, X - t + n, e, n, std::f64::consts::PI, TAU);
    p.line(X + e, X + t - n);
    p.arc(X, X + t - n, e, n, 0.0, std::f64::consts::PI);
    p.close()
}

/// `Tv` — a dome.
fn tv(e: f64, t: f64, n: f64) -> String {
    let r = X + t;
    let mut p = PathB::new();
    p.mv(X - e, r - n);
    p.arc(X, r - n, e, 2.0 * t - n, std::f64::consts::PI, TAU);
    p.line(X + e, r - n);
    p.curve(X + e, r, X + e, r, X + e - n, r);
    p.line(X - e + n, r);
    p.curve(X - e, r, X - e, r, X - e, r - n);
    p.close()
}

/// `Ev` — an arch.
fn ev(e: f64, t: f64, n: f64) -> String {
    let r = X + t;
    let o = X - t + e;
    let mut p = PathB::new();
    p.mv(X - e, o);
    p.arc(X, o, e, e, std::f64::consts::PI, TAU);
    p.line(X + e, r - n);
    p.curve(X + e, r, X + e, r, X + e - n, r);
    p.line(X - e + n, r);
    p.curve(X - e, r, X - e, r, X - e, r - n);
    p.close()
}

/// `Pv` — a shield.
fn pv_path(e: f64, t: f64, n: f64) -> String {
    let r = X - t;
    let o = X + t;
    let mut p = PathB::new();
    p.mv(X - e, r + n);
    p.curve(X - e, r - 2.0, X - e * 0.5, r - 10.0, X, r - 10.0);
    p.curve(X + e * 0.5, r - 10.0, X + e, r - 2.0, X + e, r + n);
    p.curve(X + e, X + t * 0.42, X + e * 0.62, o, X, o);
    p.curve(X - e * 0.62, o, X - e, X + t * 0.42, X - e, r + n);
    p.close()
}

/// `Av` — an egg (pinched ellipse).
fn av_path(e: f64, t: f64, n: f64) -> String {
    ur(
        |r| {
            let o = r.cos();
            let s = r.sin();
            let i = (1.0 - s) / 2.0;
            [X + o * e * (1.0 - n * i * i), X + s * t]
        },
        160,
    )
}

/// `Iv` — a teardrop (rounded apex over a round base).
fn iv(e: f64, t: f64, n: f64, r: f64) -> String {
    let o = dc(e / (n - t), -1.0, 1.0);
    let s = (1.0 - o * o).sqrt();
    let i = [X + e * s, n - e * o];
    let a = [X - e * s, n - e * o];
    let c = (i[1] - n).atan2(i[0] - X);
    let mut p = PathB::new();
    p.corner(i, [X, t], a, r);
    p.line(a[0], a[1]);
    p.arc(X, n, e, e, std::f64::consts::PI - c, c);
    p.close()
}

/// `_v` — a square with rounded corners.
fn _v(e: f64, t: f64) -> String {
    fc(
        &[[X - e, X - e], [X + e, X - e], [X + e, X + e], [X - e, X + e]],
        &[t],
    )
}

/// `Cv` — a house.
fn cv_path(e: f64, t: f64, n: f64, r: f64) -> String {
    fc(
        &[
            [X - e, X + t],
            [X + e, X + t],
            [X + e, X - t + n],
            [X, X - t],
            [X - e, X - t + n],
        ],
        &[r],
    )
}

/// `Lv` — a clover: four corner discs hull.
fn lv_path(e: f64, t: f64) -> String {
    dh(
        &[
            [X - e, X - e, t],
            [X + e, X - e, t],
            [X + e, X + e, t],
            [X - e, X + e, t],
        ],
        160,
    )
}

/// `Nv` — a leaf.
fn nv_path(e: f64, t: f64, n: f64) -> String {
    ur(
        |r| {
            let o = r.cos();
            let s = r.sin();
            [
                X + o * e * (1.0 - s * s).max(0.0).powf(n / 2.0 - 0.5) * 1.0,
                X + s * t,
            ]
        },
        160,
    )
}

/// `Fv` — a bean.
fn fv_path(e: f64, t: f64, n: f64, r: f64) -> String {
    ur(
        |o| {
            let s = |a: f64| {
                let c = (((o - a + std::f64::consts::PI) % TAU + TAU) % TAU - std::f64::consts::PI)
                    .abs();
                (-(c * c) / 0.4232).exp()
            };
            let i = 1.0 - n * s(r) + n * 0.34 * s(r + std::f64::consts::PI);
            [X + o.cos() * e * i, X + o.sin() * t * i]
        },
        160,
    )
}

/// `Dv` — a pebble (wobbly circle).
fn dv_path(e: f64, t: f64, n: f64) -> String {
    ur(
        |r| {
            let o = e * (1.0 + t * ((r * 2.0 + n).sin() * 0.6 + (r * 3.0 - n).sin() * 0.4));
            [X + r.cos() * o, X + r.sin() * o * 0.98]
        },
        160,
    )
}

/// `us` — the path-string parser/sampler: flattens `d` to points every ≤`t`px.
fn us(e: &str, t: f64) -> Vec<V2> {
    // JS tokenizes `/[MLCQZmlcqz]|-?\d*\.?\d+(?:e[-+]?\d+)?/g`.
    let mut toks: Vec<String> = Vec::new();
    let b = e.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i] as char;
        if matches!(c, 'M' | 'L' | 'C' | 'Q' | 'Z' | 'm' | 'l' | 'c' | 'q' | 'z') {
            toks.push(c.to_string());
            i += 1;
            continue;
        }
        if c == '-' || c == '.' || c.is_ascii_digit() {
            let mut j = i;
            if j < b.len() && b[j] == b'-' {
                j += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j < b.len() && b[j] == b'.' {
                j += 1;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
            }
            if j < b.len() && (b[j] == b'e' || b[j] == b'E') {
                let mut k = j + 1;
                if k < b.len() && (b[k] == b'+' || b[k] == b'-') {
                    k += 1;
                }
                let ds = k;
                while k < b.len() && b[k].is_ascii_digit() {
                    k += 1;
                }
                if k > ds {
                    j = k;
                }
            }
            if j > i {
                if let Ok(v) = e[i..j].parse::<f64>() {
                    let _ = v;
                    toks.push(e[i..j].to_string());
                }
                i = j;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    let mut n = toks.into_iter();
    let mut pend: Option<String> = None;
    macro_rules! next_tok {
        () => {
            pend.take().or_else(|| n.next())
        };
    }
    macro_rules! l {
        () => {
            next_tok!()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0)
        };
    }
    let mut r: Vec<V2> = Vec::new();
    let mut s = String::new();
    let mut cur = [0.0f64; 2];
    let mut start = [0.0f64; 2];
    loop {
        let tok = match next_tok!() {
            Some(t) => t,
            None => break,
        };
        if tok
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        {
            s = tok.to_uppercase();
        } else {
            // a number re-runs the current command (implicit lineto)
            pend = Some(tok);
        }
        match s.as_str() {
            "Z" => {
                let len = (start[0] - cur[0]).hypot(start[1] - cur[1]);
                if len > 0.01 {
                    let (a, b2, c0, d0) = (cur[0], cur[1], start[0], start[1]);
                    let p = (len / t).ceil().max(2.0) as usize;
                    for g in 1..=p {
                        let f = g as f64 / p as f64;
                        r.push([a + (c0 - a) * f, b2 + (d0 - b2) * f]);
                    }
                }
                cur = start;
            }
            "M" => {
                let f = l!();
                let h = l!();
                cur = [f, h];
                start = cur;
                r.push(cur);
                s = "L".to_owned();
            }
            "L" => {
                let f = l!();
                let h = l!();
                let (a, b2) = (cur[0], cur[1]);
                let len = (f - a).hypot(h - b2);
                let p = (len / t).ceil().max(2.0) as usize;
                for g in 1..=p {
                    let q = g as f64 / p as f64;
                    r.push([a + (f - a) * q, b2 + (h - b2) * q]);
                }
                cur = [f, h];
            }
            "Q" => {
                let (f, h, p, g) = (l!(), l!(), l!(), l!());
                let (m, y) = (cur[0], cur[1]);
                let len = (f - m).hypot(h - y) + (p - f).hypot(g - h);
                let seg = (len / t).ceil().max(2.0) as usize;
                for w in 1..=seg {
                    let w = w as f64 / seg as f64;
                    let sw = 1.0 - w;
                    r.push([
                        sw * sw * m + 2.0 * sw * w * f + w * w * p,
                        sw * sw * y + 2.0 * sw * w * h + w * w * g,
                    ]);
                }
                cur = [p, g];
            }
            _ => {
                // "C" — 6 numbers per segment
                let (f, h, p, g, m, y) = (l!(), l!(), l!(), l!(), l!(), l!());
                let (w, s2) = (cur[0], cur[1]);
                let len =
                    (f - w).hypot(h - s2) + (p - f).hypot(g - h) + (m - p).hypot(y - g);
                let seg = (len / t).ceil().max(2.0) as usize;
                for b2 in 1..=seg {
                    let b2 = b2 as f64 / seg as f64;
                    let v = 1.0 - b2;
                    r.push([
                        v * v * v * w + 3.0 * v * v * b2 * f + 3.0 * v * b2 * b2 * p + b2 * b2 * b2 * m,
                        v * v * v * s2 + 3.0 * v * v * b2 * h + 3.0 * v * b2 * b2 * g + b2 * b2 * b2 * y,
                    ]);
                }
                cur = [m, y];
            }
        }
    }
    r
}

/// `zv` — the y-bucketed span tables of a sampled outline.
struct Zv {
    top: f64,
    bottom: f64,
    n0: f64,
    o0: f64,
    span_l: Vec<f64>,
    span_r: Vec<f64>,
    outer_l: Vec<f64>,
    outer_r: Vec<f64>,
}

fn zv(e: &[V2], t: usize) -> Zv {
    let mut n = f64::INFINITY;
    let mut r = f64::NEG_INFINITY;
    for d in e {
        if d[1] < n {
            n = d[1];
        }
        if d[1] > r {
            r = d[1];
        }
    }
    let o = r - n;
    let s = |d: usize| n + o * (d as f64 + 0.5) / t as f64;
    let mut il = vec![X; t];
    let mut ir = vec![X; t];
    let mut ol = vec![X; t];
    let mut orr = vec![X; t];
    for d in 0..t {
        let f = s(d);
        let (mut h, mut p, mut g, mut m) = (
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        );
        for y in 0..e.len() {
            let w = e[y];
            let s2 = e[(y + 1) % e.len()];
            if (w[1] <= f) == (s2[1] <= f) {
                continue;
            }
            let b = w[0] + (s2[0] - w[0]) * (f - w[1]) / (s2[1] - w[1]);
            if b <= X {
                if b > h {
                    h = b;
                }
            } else if b < p {
                p = b;
            }
            if b < g {
                g = b;
            }
            if b > m {
                m = b;
            }
        }
        il[d] = if h.is_finite() { h } else { X };
        ir[d] = if p.is_finite() { p } else { X };
        ol[d] = if g.is_finite() { g } else { X };
        orr[d] = if m.is_finite() { m } else { X };
    }
    Zv {
        top: n,
        bottom: r,
        n0: n,
        o0: o,
        span_l: il,
        span_r: ir,
        outer_l: ol,
        outer_r: orr,
    }
}

impl Zv {
    fn at(d: &[f64], f: &[f64], n0: f64, o0: f64, t: usize, h: f64) -> V2 {
        let p = dc((h - n0) / o0 * t as f64 - 0.5, 0.0, t as f64 - 1.0);
        let g = p.floor() as usize;
        let m = p - g as f64;
        let y = (g + 1).min(t - 1);
        [d[g] + (d[y] - d[g]) * m, f[g] + (f[y] - f[g]) * m]
    }
    fn span_at(&self, h: f64) -> V2 {
        Zv::at(&self.span_l, &self.span_r, self.n0, self.o0, self.span_l.len(), h)
    }
    fn outer_at(&self, h: f64) -> V2 {
        Zv::at(&self.outer_l, &self.outer_r, self.n0, self.o0, self.outer_l.len(), h)
    }
}

const RL: usize = 96; // `Rl`

/// `jv` — the 96-point polar outline ring around `x`.
fn jv(e: &[V2]) -> Vec<V2> {
    (0..RL)
        .map(|n| {
            let r = n as f64 / RL as f64 * TAU;
            let o = r.cos();
            let s = r.sin();
            let mut i = 0.0f64;
            for a in 0..e.len() {
                let c = e[a];
                let u = e[(a + 1) % e.len()];
                let l = c[0] - X;
                let d = c[1] - X;
                let f = u[0] - X;
                let h = u[1] - X;
                let p = (f - l) * s - (h - d) * o;
                if p.abs() < 1e-9 {
                    continue;
                }
                let g = (l * s - d * o) / -p;
                if g < 0.0 || g > 1.0 {
                    continue;
                }
                let m = (l + (f - l) * g) * o + (d + (h - d) * g) * s;
                if m > i {
                    i = m;
                }
            }
            [X + o * i, X + s * i]
        })
        .collect()
}

/// `qv` — scale + translate every number of a path string around `x`.
fn qv(e: &str, t: f64, n: f64, r: f64) -> String {
    let mut out = String::with_capacity(e.len());
    let mut o = false;
    let bytes = e.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i] as char;
        let is_num = c.is_ascii_digit() || c == '-' || c == '.';
        if is_num {
            let mut j = i;
            while j < bytes.len() {
                let d = bytes[j] as char;
                if d.is_ascii_digit()
                    || d == '.'
                    || d == '-'
                    || d == '+'
                    || d == 'e'
                    || d == 'E'
                {
                    if (d == '-' || d == '+')
                        && j > i
                        && !matches!(bytes[j - 1] as char, 'e' | 'E')
                    {
                        break;
                    }
                    j += 1;
                } else {
                    break;
                }
            }
            let v: f64 = e[i..j].parse().unwrap_or(0.0);
            o = !o;
            let v2 = v + if o { n } else { r };
            out += &format!("{}", te(X + (v2 - X) * t));
            i = j;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// `Hv` — normalization (scale to `Bv`, recenter).
fn hv(e: &[V2]) -> (f64, f64, f64) {
    let (mut t, mut n, mut r, mut o) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for c in e {
        if c[0] < t {
            t = c[0];
        }
        if c[0] > n {
            n = c[0];
        }
        if c[1] < r {
            r = c[1];
        }
        if c[1] > o {
            o = c[1];
        }
    }
    let s = X - (t + n) / 2.0;
    let i = X - (r + o) / 2.0;
    (dc(BV / (n - t).max(o - r), 0.9, 1.35), s, i)
}

/// `Vv` — normalize a path to the standard mark box.
fn vv_norm(e: &str, t: &[V2]) -> String {
    let (k, dx, dy) = hv(t);
    if (k - 1.0).abs() < 0.005 && dx.abs() < 0.5 && dy.abs() < 0.5 {
        e.to_owned()
    } else {
        qv(e, k, dx, dy)
    }
}

/// A baked outline: path, ring and span tables (`hc`'s record).
pub(crate) struct Outline {
    pub path: String,
    pub ring: Vec<V2>,
    pub top: f64,
    pub bottom: f64,
    pub belt_radius: f64,
    zv: Zv,
}

impl Outline {
    pub(crate) fn span_at(&self, h: f64) -> V2 {
        self.zv.span_at(h)
    }
}

/// `hc` — bake an outline record from path data.
fn hc(e: &str) -> Outline {
    let t = vv_norm(e, &us(e, 4.0));
    let n = us(&t, 4.0);
    let z = zv(&n, 160);
    let mut a = 0.0f64;
    let mut c = z.top;
    while c <= z.bottom {
        let [u, l] = z.outer_at(c);
        a = a.max((l - u) / 2.0);
        c += 2.0;
    }
    Outline {
        ring: jv(&n),
        top: te(z.top),
        bottom: te(z.bottom),
        belt_radius: te(a),
        path: t,
        zv: z,
    }
}

/// `Gv` — the outline generator table (path data in mark-box coords).
fn gv(e: &str) -> String {
    match e {
        "pebble" => dv_path(108.0, 0.075, 1.1),
        "bean" => fv_path(94.0, 112.0, 0.34, std::f64::consts::PI),
        "egg" => av_path(98.0, 113.0, 0.22),
        "squircle" => cl_path(107.0, 107.0, 3.2),
        "square" => _v(107.0, 28.0),
        "tablet" => vv(114.0, 74.0),
        "capsule" => sv(72.0, 113.0),
        "cylinder" => mv_path(94.0, 110.0, 48.0),
        "hex" => lh(114.0, 6, &[20.0], std::f64::consts::PI / 6.0),
        "gem" => cl_path(112.0, 113.0, 1.5),
        "shield" => pv_path(98.0, 108.0, 30.0),
        "dome" => tv(114.0, 82.0, 26.0),
        "arch" => ev(76.0, 113.0, 42.0),
        "cloud" => dh(
            &[
                [X - 62.0, X + 26.0, 56.0],
                [X + 62.0, X + 26.0, 54.0],
                [X, X + 34.0, 62.0],
                [X - 24.0, X - 30.0, 62.0],
                [X + 38.0, X - 26.0, 54.0],
            ],
            160,
        ),
        "leaf" => nv_path(88.0, 113.0, 1.5),
        "clover" => lv_path(38.0, 58.0),
        "house" => cv_path(100.0, 110.0, 78.0, 29.0),
        // `Rv`/`Ov`/`$v` — heart/star6/sparkle are only reached through the
        // atlas (their seats are `front`, no ring or path needed).
        _ => String::new(),
    }
}

thread_local! {
    static ZL_CACHE: RefCell<HashMap<&'static str, Rc<Outline>>> = RefCell::new(HashMap::new());
    static JL_CACHE: RefCell<HashMap<&'static str, Rc<BodyInfo>>> = RefCell::new(HashMap::new());
    static ATLAS_CACHE: RefCell<HashMap<&'static str, Option<Rc<Atlas>>>> = RefCell::new(HashMap::new());
}

/// `Zr` — the cached outline of a named body.
fn zr_outline(e: &'static str) -> Rc<Outline> {
    ZL_CACHE.with(|c| {
        c.borrow_mut()
            .entry(e)
            .or_insert_with(|| Rc::new(hc(&gv(e))))
            .clone()
    })
}

/// `mt` — a sampled body outline.
fn mt(e: &'static str, t: f64) -> Vec<V2> {
    us(&zr_outline(e).path, t)
}

/// `Ol` — sample any path string.
fn ol(e: &str) -> Vec<V2> {
    us(e, 4.0)
}

/// `fh` — a teardrop outline.
pub(crate) fn fh(e: f64, t: f64) -> Outline {
    hc(&iv(e, X - 114.0, X + 26.0, t))
}

/// `Wv` — a wedge outline.
fn wv(e: f64, t: f64) -> Outline {
    hc(&lh(130.0, 3, &[t, e, e], -std::f64::consts::PI / 2.0))
}

/// `fa` — the ring bbox of a body.
fn fa(e: &'static str) -> (f64, f64, f64, f64) {
    let (mut t, mut n, mut r, mut o) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for &[s, i] in &zr_outline(e).ring {
        if s < t {
            t = s;
        }
        if s > n {
            n = s;
        }
        if i < r {
            r = i;
        }
        if i > o {
            o = i;
        }
    }
    (t, n, r, o)
}

/// `gh` — the row-width measure of a sampled outline.
fn gh(e: Vec<V2>) -> impl Fn(f64) -> f64 + 'static {
    const GI: usize = 64;
    let go = 1e-5;
    let t = e.len();
    let (mut n, mut r) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in &e {
        let i = p[1];
        if i < n {
            n = i;
        }
        if i > r {
            r = i;
        }
    }
    n -= go;
    r += go;
    let e2: Vec<V2> = e.clone();
    let o = move |i: f64| -> i64 {
        (((i - n) / (r - n) * GI as f64).floor() as i64).min(GI as i64 - 1)
    };
    let mut s: Vec<Vec<usize>> = vec![Vec::new(); GI];
    for i in 0..t {
        let a = e[i][1];
        let c = e[(i + 1) % t][1];
        let u = o(a.max(c) + go);
        let lo = o(a.min(c) - go);
        for l in lo.max(0)..=u.min(GI as i64 - 1) {
            s[l as usize].push(i);
        }
    }
    move |i: f64| {
        let (mut a, mut c) = (f64::INFINITY, f64::NEG_INFINITY);
        if !(i >= n && i <= r) {
            return c - a;
        }
        let bucket = o(i);
        if bucket < 0 || bucket as usize >= GI {
            return c - a;
        }
        for &u in &s[bucket as usize] {
            let (l, d) = (e2[u][0], e2[u][1]);
            let (f, h) = {
                let q = e2[(u + 1) % t];
                (q[0], q[1])
            };
            if (h - d).abs() < 1e-9 {
                if (d - i).abs() < 1e-6 {
                    a = a.min(l.min(f));
                    c = c.max(l.max(f));
                }
                continue;
            }
            if (d <= i && h >= i) || (h <= i && d >= i) {
                let p = l + (f - l) * (i - d) / (h - d);
                if p < a {
                    a = p;
                }
                if p > c {
                    c = p;
                }
            }
        }
        c - a
    }
}

/// `pa` — a body's normalized half-width as a function of y.
fn pa(e: &[V2]) -> Rc<dyn Fn(f64) -> f64> {
    let t = gh(e.to_vec());
    let (mut n, mut r) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in e {
        let o = p[1];
        if o < n {
            n = o;
        }
        if o > r {
            r = o;
        }
    }
    Rc::new(move |o: f64| {
        let s = dc(X - o * X, n, r);
        (t(s) / 2.0 / X).max(0.02)
    })
}

/// `kh` — `pa` of a named body.
fn kh(e: &'static str) -> Rc<dyn Fn(f64) -> f64> {
    pa(&mt(e, 4.0))
}

/// `Jt` — an outline ring as normalized 3D points (z=0).
fn jt(e: &[V2]) -> Vec<V3> {
    e.iter()
        .map(|&[t, n]| [(t - X) / X, -(n - X) / X, 0.0])
        .collect()
}

/// `Gl` — `Fl` inscribed balls along an axis (the pill bodies).
fn gl_balls(e: &'static str, t: &str) -> Vec<[f64; 4]> {
    const FL: usize = 33;
    let (x0, x1, y0, y1) = fa(e);
    let i = (x1 - x0) / 2.0 / X;
    let a = (y1 - y0) / 2.0 / X;
    let c = if t == "x" { a } else { i };
    let u = ((if t == "x" { i } else { a }) - c).max(0.0);
    let l = -u;
    (0..FL)
        .map(|f| {
            let h = l + (u - l) * (f as f64 / (FL - 1) as f64);
            if t == "x" {
                [h, 0.0, 0.0, c]
            } else {
                [0.0, h, 0.0, c]
            }
        })
        .collect()
}

/// `tS` — the half extents of a ball chain.
fn ts_extents(e: &[[f64; 4]]) -> (f64, f64) {
    let (mut t, mut n) = (0.55f64, 0.35f64);
    for &[r, _, o, s] in e {
        t = t.max(r.abs() + s);
        n = n.max(o.abs() + s);
    }
    (t, n)
}

/// `nS` — the inscribed ball packing of a body outline (the bean's puffs).
fn ns_balls(e: &'static str) -> Vec<[f64; 4]> {
    const XV: usize = 40;
    let t = mt(e, 4.0);
    let (mut n, mut r, mut o, mut s) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for &[l, d] in &t {
        n = n.min(l);
        r = r.max(l);
        o = o.min(d);
        s = s.max(d);
    }
    let inside = |l: f64, d: f64| -> bool {
        let mut f = false;
        let h = t.len();
        let mut g = h.wrapping_sub(1);
        for p in 0..h {
            let (m, y) = (t[p][0], t[p][1]);
            let (w, s2) = (t[g][0], t[g][1]);
            if (y > d) != (s2 > d) && l < m + (w - m) * (d - y) / (s2 - y) {
                f = !f;
            }
            g = p;
        }
        f
    };
    let a = (r - n).max(s - o) / XV as f64;
    let mut c: Vec<[f64; 3]> = Vec::new();
    let mut l = o + a / 2.0;
    while l < s {
        let mut d = n + a / 2.0;
        while d < r {
            if inside(d, l) {
                let mut f = f64::INFINITY;
                for &[p, g] in &t {
                    f = f.min((p - d) * (p - d) + (g - l) * (g - l));
                }
                let h = f.sqrt();
                if h > a {
                    c.push([d, l, h]);
                }
            }
            d += a;
        }
        l += a;
    }
    c.sort_by(|a2, b2| b2[2].partial_cmp(&a2[2]).unwrap_or(std::cmp::Ordering::Equal));
    let mut u: Vec<[f64; 3]> = Vec::new();
    for l in &c {
        let dominated = u.iter().any(|d| {
            ((l[0] - d[0]).hypot(l[1] - d[1])) + l[2] <= d[2] + a * 0.25
        });
        if !dominated {
            u.push(*l);
        }
    }
    u.iter()
        .map(|l| [(l[0] - X) / X, -(l[1] - X) / X, 0.0, l[2] / X])
        .collect()
}

/// `Qv` — the puff at a point: the nearest covering ball.
fn qv_puff(e: &[[f64; 4]], t: f64, n: f64) -> Puff {
    let mut r = e[0];
    let (mut o, mut s, mut i) = (f64::NEG_INFINITY, f64::INFINITY, false);
    for &a in e {
        let c = (t - a[0]).hypot(n - a[1]);
        if c < a[3] {
            let u = a[2] + (a[3] * a[3] - c * c).sqrt();
            if !i || u > o {
                i = true;
                o = u;
                r = a;
            }
        } else if !i && c - a[3] < s {
            s = c - a[3];
            r = a;
        }
    }
    Puff {
        x: r[0],
        y: r[1],
        z: r[2],
        r: r[3],
    }
}

// ---------------------------------------------------------------------------
// seats and bodies (`Seat`, `mh`/`Bn`/`Bl`/`Hn`/`ph`/`Hl`, `Yv`/`Cs`,
// `sS`/`iS`)
// ---------------------------------------------------------------------------

/// A sphere-surface ball of a wrap seat's puff table.
#[derive(Clone, Copy)]
pub(crate) struct Puff {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub r: f64,
}

/// `seat` — how an eye loop lands on a body.
pub(crate) enum Seat {
    /// `front` — the eye plane at a fixed z.
    Front { face_z: f64 },
    /// `loft` — the domed front of a lathed body.
    Loft { surface_z: Rc<dyn Fn(f64, f64) -> f64> },
    /// `wrap` — wrapped around a lathed body.
    Wrap(Wrap),
    /// `wrapX` — wrapped around a sideways pill.
    WrapX(WrapX),
}

pub(crate) struct Wrap {
    pub wrap_r: f64,
    pub wrap_y0: f64,
    pub wrap_y1: f64,
    pub eye_y: f64,
    pub radius_at: Option<Rc<dyn Fn(f64) -> f64>>,
    pub puff_at: Option<Rc<dyn Fn(f64, f64) -> Puff>>,
}

pub(crate) struct WrapX {
    pub wrap_r: f64,
    pub wrap_x0: f64,
    pub wrap_x1: f64,
    pub radius_at_x: Rc<dyn Fn(f64) -> f64>,
}

/// The body's render record (`Cs`'s output, `silhouette` dropped).
pub(crate) struct BodyInfo {
    pub size: f64,
    pub seat: Option<Seat>,
}

/// `mh` — a wrap seat from a body's own outline.
fn mh(e: &'static str, t: Vec<[f64; 4]>) -> Seat {
    let n = zr_outline(e);
    let r = -(n.bottom - X) / X;
    let o = -(n.top - X) / X;
    let s = kh(e);
    let i = (o - 0.02).min((r + 0.02).max(0.0));
    Seat::Wrap(Wrap {
        wrap_r: s(i),
        wrap_y0: r,
        wrap_y1: o,
        eye_y: i,
        radius_at: Some(s),
        puff_at: if t.is_empty() {
            None
        } else {
            let t = Rc::new(t);
            Some(Rc::new(move |a: f64, c: f64| qv_puff(&t, a, c)))
        },
    })
}

/// `Bn` — a revolve-ish body (wrap seat over its own outline).
fn bn(e: f64, t: &'static str) -> BodyInfo {
    BodyInfo {
        size: e,
        seat: Some(mh(t, vec![])),
    }
}

/// `Bl` — a wrap body with a custom `eyeY` blend.
fn bl(e: &'static str, t: impl Fn(f64, f64) -> f64) -> BodyInfo {
    let n = zr_outline(e);
    let r = -(n.top - X) / X;
    let o = -(n.bottom - X) / X;
    BodyInfo {
        size: 1.17,
        seat: Some(Seat::Wrap(Wrap {
            wrap_r: n.belt_radius / X,
            wrap_y0: o,
            wrap_y1: r,
            eye_y: t(r, o),
            radius_at: Some(kh(e)),
            puff_at: None,
        })),
    }
}

const UV: f64 = 0.36; // `Uv` — the extrusion seat depth
const KV: f64 = 0.22; // `Kv` — extrusion bevel (silhouette only, kept for parity)

/// `Hn`/`ph` — an extruded front-face body (`Uv` face; `ph`'s `Kv` bevel is
/// silhouette-only and dropped with it).
fn hn(e: f64, face_z: f64) -> BodyInfo {
    let _ = KV;
    BodyInfo {
        size: e,
        seat: Some(Seat::Front { face_z }),
    }
}

/// `sS`/`iS` — the loft: lathed rings from a 2D ring + the surface function.
struct LoftRings {
    rings: Vec<Vec<V3>>,
    center: V2,
    depth: f64,
}

fn ss(e: &[V3], t: f64) -> LoftRings {
    const ZL: usize = 160;
    const DL: usize = 16;
    const HH: f64 = 0.72;
    let n = e.len() as f64 / ZL as f64;
    let r: Vec<V3> = if n > 1.0 {
        (0..ZL).map(|d| e[(d as f64 * n).floor() as usize]).collect()
    } else {
        e.to_vec()
    };
    let (mut o, mut s, mut i, mut a) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for l in &r {
        o = o.min(l[0]);
        s = s.max(l[0]);
        i = i.min(l[1]);
        a = a.max(l[1]);
    }
    let c = [(o + s) / 2.0, (i + a) / 2.0];
    let mut u = Vec::with_capacity(DL + 1);
    for l in 0..=DL {
        let d = -std::f64::consts::PI / 2.0 + std::f64::consts::PI * l as f64 / DL as f64;
        let f = d.cos().max(0.0).powf(HH);
        let h = t * d.sin();
        u.push(
            r.iter()
                .map(|p| {
                    [
                        c[0] + (p[0] - c[0]) * f,
                        c[1] + (p[1] - c[1]) * f,
                        h,
                    ]
                })
                .collect::<Vec<_>>(),
        );
    }
    LoftRings {
        rings: u,
        center: c,
        depth: t,
    }
}

fn is(lr: &LoftRings) -> Rc<dyn Fn(f64, f64) -> f64> {
    const QN: usize = 180;
    const HH: f64 = 0.72;
    let r = &lr.rings[lr.rings.len() / 2];
    let t = lr.center;
    let n = lr.depth;
    let mut o = [0.0f64; QN];
    for i in 0..QN {
        let a = (i as f64 + 0.5) / QN as f64 * TAU;
        let (c, u) = (a.cos(), a.sin());
        let mut l = 0.0f64;
        for d in 0..r.len() {
            let f = r[d];
            let h = r[(d + 1) % r.len()];
            let p = h[0] - f[0];
            let g = h[1] - f[1];
            let m = c * g - u * p;
            if m.abs() < 1e-9 {
                continue;
            }
            let y = f[0] - t[0];
            let w = f[1] - t[1];
            let s2 = (y * g - w * p) / m;
            let b = (y * u - w * c) / m;
            if s2 > l && b >= 0.0 && b <= 1.0 {
                l = s2;
            }
        }
        o[i] = l;
    }
    let s_fn = move |i: f64, a: f64| -> f64 {
        let c = i - t[0];
        let u = a - t[1];
        let l = c.hypot(u);
        if l < 1e-8 {
            return 0.0;
        }
        let f = ((u.atan2(c) + TAU) % TAU) / TAU * QN as f64;
        let h = (f.floor() as usize) % QN;
        let p = f - f.floor();
        let g = o[h];
        let m = o[(h + 1) % QN];
        (l / (1e-8f64.max(g * (1.0 - p) + m * p))).min(1.0)
    };
    Rc::new(move |i: f64, a: f64| {
        n * (1.0 - s_fn(i, a).powf(2.0 / HH)).max(0.0).sqrt()
    })
}

/// `Hl` — a lofted body (cloud, heartChubby).
fn hl(e: f64, t: Vec<V3>, n: f64) -> BodyInfo {
    let r = ss(&t, n);
    BodyInfo {
        size: e,
        seat: Some(Seat::Loft {
            surface_z: is(&r),
        }),
    }
}

/// `Yv` — the body table (seats only; silhouettes were atlas-baked offline).
fn body_info(e: &str) -> Option<BodyInfo> {
    Some(match e {
        "pebble" => bn(1.2168, "pebble"),
        "bean" => BodyInfo {
            size: 1.17,
            seat: Some(mh("bean", ns_balls("bean"))),
        },
        "egg" => bn(1.17, "egg"),
        "squircle" => BodyInfo {
            size: 1.17,
            seat: Some(Seat::Front { face_z: 0.28 }),
        },
        "tablet" => {
            let (x0, x1, y0, y1) = fa("tablet");
            let o = (x1 - x0) / 2.0 / X;
            let s = (y1 - y0) / 2.0 / X;
            let i = (o - s).max(0.0);
            BodyInfo {
                size: 1.3923,
                seat: Some(Seat::WrapX(WrapX {
                    wrap_r: s,
                    wrap_x0: -o + 0.02,
                    wrap_x1: o - 0.02,
                    radius_at_x: Rc::new(move |a: f64| {
                        let c = a.abs() - i;
                        if c <= 0.0 {
                            s
                        } else {
                            (s * s - c * c).max(0.0).sqrt()
                        }
                    }),
                })),
            }
        }
        "capsule" => {
            let e2 = gl_balls("capsule", "y");
            let (_, _, y0, y1) = fa("capsule");
            let r = -(y1 - X) / X;
            let o = -(y0 - X) / X;
            let (hx, hz) = ts_extents(&e2);
            BodyInfo {
                size: 1.17,
                seat: Some(Seat::Wrap(Wrap {
                    wrap_r: (0.35f64).max((hx + hz) * 0.5),
                    wrap_y0: r,
                    wrap_y1: o,
                    eye_y: (r + o) * 0.5,
                    radius_at: None,
                    puff_at: None,
                })),
            }
        }
        "cylinder" => bl("cylinder", |e, t| (t + e) * 0.5),
        "hex" => hn(1.2519, UV),
        "gem" => bn(1.17, "gem"),
        "crystal" => BodyInfo {
            size: 1.3806,
            seat: Some(Seat::Front {
                face_z: 0.64 * (3.0f64.sqrt() / 2.0),
            }),
        },
        "wedge" => {
            let e2 = wv(60.0, 80.0);
            let t = ol(&e2.path);
            let [n, r] = e2.span_at(e2.top + (e2.bottom - e2.top) * 0.5);
            let o = -(e2.top - X) / X;
            let s = -(e2.bottom - X) / X;
            BodyInfo {
                size: 1.359072,
                seat: Some(Seat::Wrap(Wrap {
                    wrap_r: (0.2f64).max((r - n) / 2.0 / X),
                    wrap_y0: s,
                    wrap_y1: o,
                    eye_y: o + (s - o) * 0.5,
                    radius_at: Some(pa(&t)),
                    puff_at: None,
                })),
            }
        }
        "shield" => bn(1.17, "shield"),
        "dome" => bn(1.17, "dome"),
        "arch" => bl("arch", |e, t| e + (t - e) * 0.42),
        "cloud" => hl(1.3455, jt(&mt("cloud", 1.0)), 0.72),
        "teardrop" => {
            let e2 = fh(88.0, 48.0);
            let t = ol(&e2.path);
            let [n, r] = e2.span_at(e2.top + (e2.bottom - e2.top) * 0.62);
            let o = -(e2.bottom - X) / X;
            let s = -(e2.top - X) / X;
            BodyInfo {
                size: 1.318122,
                seat: Some(Seat::Wrap(Wrap {
                    wrap_r: (0.2f64).max((r - n) / 2.0 / X),
                    wrap_y0: o,
                    wrap_y1: s,
                    eye_y: (o + s) * 0.5,
                    radius_at: Some(pa(&t)),
                    puff_at: None,
                })),
            }
        }
        "leaf" => bn(1.17, "leaf"),
        "square" => hn(1.0413, UV),
        "starFlower" => hn(1.3923, UV),
        "sparkle" => hn(1.521, UV),
        "clover" => hn(1.0881, UV),
        "heartChubby" => {
            let pts: Vec<V3> = HEART_OUTLINE
                .iter()
                .map(|&[px, py]| [px, py, 0.0])
                .collect();
            hl(1.3572, pts, 0.62)
        }
        "house" => hn(1.17, UV),
        "star6" => hn(1.4976, 0.3),
        _ => return None,
    })
}

/// `Cs` — the cached body record.
pub(crate) fn cs(e: &'static str) -> Option<Rc<BodyInfo>> {
    JL_CACHE.with(|c| {
        if let Some(b) = c.borrow().get(e) {
            return Some(b.clone());
        }
        let info = body_info(e).map(Rc::new);
        if let Some(info) = info {
            c.borrow_mut().insert(e, info.clone());
            Some(info)
        } else {
            None
        }
    })
}

// ---------------------------------------------------------------------------
// the rotation + projector (`yS`, `xh`, `pc`) and the seat projectors
// (`vS` → `SS`/`MS`/`TS`/`ES`), `gS`
// ---------------------------------------------------------------------------

/// `Rot` — yaw/pitch/roll in degrees (`Ch`'s output).
#[derive(Clone, Copy)]
pub(crate) struct Rot {
    pub yaw: f64,
    pub pitch: f64,
    pub roll: f64,
}

const AS: f64 = 0.2; // `aS`
const CS: f64 = 0.36; // `cS`
const US: f64 = 1.18; // `uS`

/// `pc` — the projection radius of a body at display size `t`.
fn pc(e: &BodyInfo, t: f64) -> f64 {
    e.size.max(AS) * (CS * t / US)
}

/// `yi` — Rodrigues terms.
fn yi(e: f64) -> [f64; 3] {
    [e.cos(), e.sin(), 1.0 - e.cos()]
}

/// `yS` — yaw→pitch→roll rotation of a 3D point.
fn ys(e: f64, t: f64, n: f64) -> impl Fn(V3) -> V3 {
    let [r, o, _s] = yi(e);
    let [i, a, _c] = yi(t);
    let [u, l, _d] = yi(n);
    move |[f, h, p]| {
        let g = f * r + p * o;
        let m = h * r + h * _s;
        let y = p * r - f * o;
        let w = g * i + g * _c;
        let s2 = m * i - y * a;
        let b = y * i + m * a;
        [w * u - s2 * l, s2 * u + w * l, b * u + b * _d]
    }
}

/// `xh` — the body's projector at a pose (`{rotate, project, origin, radius}`).
pub(crate) struct Projector {
    pub rotate: Box<dyn Fn(V3) -> V3>,
    pub origin: V2,
    pub radius: f64,
}

impl Projector {
    pub fn project(&self, i: V3) -> V2 {
        let a = (self.rotate)(i);
        [
            self.origin[0] + self.radius * a[0],
            self.origin[1] - self.radius * a[1],
        ]
    }
}

fn xh(e: &BodyInfo, t: &Rot, n: f64) -> Projector {
    let r = n / 2.0;
    let o = pc(e, n);
    let s = ys(
        t.yaw * std::f64::consts::PI / 180.0,
        t.pitch * std::f64::consts::PI / 180.0,
        t.roll * std::f64::consts::PI / 180.0,
    );
    Projector {
        rotate: Box::new(s),
        origin: [r, r],
        radius: o,
    }
}

/// `vh` — keep only finite loops of ≥3 points.
fn vh(e: Vec<V2>) -> Option<Vec<V2>> {
    if e.len() < 2 || !e[0][0].is_finite() || !e[0][1].is_finite() {
        return None;
    }
    let n: Vec<V2> = e
        .into_iter()
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .collect();
    if n.len() >= 3 {
        Some(n)
    } else {
        None
    }
}

/// `SS` — the `front` seat: eye loops on a plane facing the viewer.
fn ss_seat(e: f64, t: &EyeLoops, r: &Projector) -> Vec<Vec<V2>> {
    if !((r.rotate)([0.0, 0.0, 1.0])[2] > 0.03) {
        return vec![];
    }
    let s = t.shift_y.sin();
    t.loops
        .iter()
        .filter(|i| i.len() >= 3)
        .map(|i| {
            i.iter()
                .map(|a| r.project([a[0], a[1] - s, e]))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// `MS` — the `loft` seat: eye loops on the domed surface.
fn ms_seat(e: &Rc<dyn Fn(f64, f64) -> f64>, t: &EyeLoops, pr: &Projector) -> Vec<Vec<V2>> {
    let r = &pr.rotate;
    if !(r([0.0, 0.0, 1.0])[2] >= 0.05) {
        return vec![];
    }
    let i = t.shift_y.sin();
    let mut a = Vec::new();
    for c in &t.loops {
        if c.len() < 3 {
            continue;
        }
        let u: Vec<V2> = c.iter().map(|y| [y[0], y[1] - i]).collect();
        let (mut l, mut d) = (0.0, 0.0);
        for y in &u {
            l += y[0];
            d += y[1];
        }
        l /= u.len() as f64;
        d /= u.len() as f64;
        let f = 0.004;
        let h = (e(l + f, d) - e(l - f, d)) / (2.0 * f);
        let p = (e(l, d + f) - e(l, d - f)) / (2.0 * f);
        let g = (h * h + p * p + 1.0).sqrt();
        if r([-h / g, -p / g, 1.0 / g])[2] < 0.12 {
            continue;
        }
        let m = e(l, d);
        a.push(
            u.iter()
                .map(|y| {
                    pr.project([y[0], y[1], m + (y[0] - l) * h + (y[1] - d) * p])
                })
                .collect::<Vec<_>>(),
        );
    }
    a
}

/// `TS` — the `wrap` seat: eye loops wrapped around a lathed body.
fn ts_seat(e: &Wrap, t: &EyeLoops, pr: &Projector) -> Vec<Vec<V2>> {
    let r = &pr.rotate;
    let o = |i: V3| pr.project(i);
    let (s, i, a) = (e.wrap_r, e.wrap_y0, e.wrap_y1);
    let l = if e.radius_at.is_some() { 0.02 } else { 0.06 };
    let d = (a - l).min((i + l).max(e.eye_y - t.shift_y));
    let f = |b: f64| a.min(i.max(b));
    let c = &e.radius_at;
    let u = &e.puff_at;
    let h = |b: &[V2]| -> Vec<V2> {
        let mut v = Vec::new();
        for pt in b {
            let aa = f(d + pt[1]);
            let fr = c.as_ref().map_or(s, |c| c(aa));
            let n = dc(pt[0] / fr.max(0.12), -2.6, 2.6);
            let mut rr = 0.0;
            if let Some(c) = c {
                rr = -(c((aa + 0.012).min(a)) - c((aa - 0.012).max(i))) / (2.0 * 0.012);
            }
            if r([n.sin(), rr, n.cos()])[2] >= 0.05 {
                v.push(o([fr * n.sin(), aa, fr * n.cos()]));
            }
        }
        v
    };
    let p = |b: &[V2]| -> Vec<V2> {
        let Some(u) = u else {
            return vec![];
        };
        if b.is_empty() {
            return vec![];
        }
        let (mut v, mut ee) = (0.0, 0.0);
        for r2 in b {
            v += r2[0];
            ee += r2[1];
        }
        let aa = u(v / b.len() as f64, f(d + ee / b.len() as f64));
        let ff = r([0.0, 0.0, 1.0])[2];
        let mut nn = Vec::new();
        for r2 in b {
            let bb = r2[0] - aa.x;
            let pp = f(d + r2[1]) - aa.y;
            let ii = bb.hypot(pp);
            let mut uu = (ii / aa.r).min(std::f64::consts::PI - 0.02);
            let oo = if ii > 1e-9 { bb / ii } else { 1.0 };
            let hh = if ii > 1e-9 { pp / ii } else { 0.0 };
            let ll = r([oo, hh, 0.0])[2];
            let j = |k: f64| ll * k.sin() + ff * k.cos();
            if j(uu) < 0.06 && ff > 0.06 {
                let (mut k, mut te2) = (0.0, uu);
                for _ in 0..20 {
                    let z = (k + te2) / 2.0;
                    if j(z) >= 0.06 {
                        k = z;
                    } else {
                        te2 = z;
                    }
                }
                uu = k;
            }
            let jj = aa.r * uu.sin();
            let fe = [oo * uu.sin(), hh * uu.sin(), uu.cos()];
            if r(fe)[2] >= 0.05 {
                nn.push(o([aa.x + oo * jj, aa.y + hh * jj, aa.z + aa.r * uu.cos()]));
            }
        }
        nn
    };
    let (mut g, mut m) = (f64::INFINITY, f64::NEG_INFINITY);
    for b in &t.loops {
        for v in b {
            if v[1] < g {
                g = v[1];
            }
            if v[1] > m {
                m = v[1];
            }
        }
    }
    let mut y = 0.0;
    if m - g <= a - i {
        if d + g < i {
            y = i - (d + g);
        } else if d + m > a {
            y = a - (d + m);
        }
    }
    let w: Vec<Vec<V2>> = if y == 0.0 {
        t.loops.clone()
    } else {
        t.loops
            .iter()
            .map(|b| b.iter().map(|v| [v[0], v[1] + y]).collect())
            .collect()
    };
    let s_fn: &dyn Fn(&[V2]) -> Vec<V2> = if u.is_some() { &p } else { &h };
    w.iter()
        .map(|b| s_fn(b))
        .filter(|b| b.len() > 2)
        .collect()
}

/// `ES` — the `wrapX` seat: eye loops wrapped around a sideways pill.
fn es_seat(e: &WrapX, t: &EyeLoops, pr: &Projector) -> Vec<Vec<V2>> {
    let r = &pr.rotate;
    let o = |i: V3| pr.project(i);
    let (s, i, a) = (e.wrap_r, e.wrap_x0, e.wrap_x1);
    let c = &e.radius_at_x;
    let u = dc(-t.shift_y / s, -2.4, 2.4);
    let (mut l, mut d) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in &t.loops {
        for g in p {
            if g[0] < l {
                l = g[0];
            }
            if g[0] > d {
                d = g[0];
            }
        }
    }
    let mut f = 0.0;
    if d - l <= a - i {
        if l < i {
            f = i - l;
        } else if d > a {
            f = a - d;
        }
    }
    let w: Vec<Vec<V2>> = if f == 0.0 {
        t.loops.clone()
    } else {
        t.loops
            .iter()
            .map(|p| p.iter().map(|g| [g[0] + f, g[1]]).collect())
            .collect()
    };
    w.iter()
        .map(|p| {
            let mut g = Vec::new();
            for m in p {
                let y = m[0].min(a).max(i);
                let ww = c(y);
                let ss = u + dc(m[1] / s.max(0.12), -2.6, 2.6);
                let b = 0.012;
                let v = -(c((y + b).min(a)) - c((y - b).max(i))) / (2.0 * b);
                if r([v, ss.sin(), ss.cos()])[2] >= 0.05 {
                    g.push(o([y, ww * ss.sin(), ww * ss.cos()]));
                }
            }
            g
        })
        .filter(|p| p.len() > 2)
        .collect()
}

/// `vS` — dispatch to the seat's projector.
fn vs(e: &Seat, t: &EyeLoops, n: &Projector) -> Vec<Vec<V2>> {
    match e {
        Seat::Front { face_z } => ss_seat(*face_z, t, n),
        Seat::Loft { surface_z } => ms_seat(surface_z, t, n),
        Seat::Wrap(w) => ts_seat(w, t, n),
        Seat::WrapX(w) => es_seat(w, t, n),
    }
}

/// `gS` — the body's eye outlines in sprite space.
pub(crate) fn gs(info: &BodyInfo, rot: &Rot, n: f64, loops: &EyeLoops) -> Vec<Vec<V2>> {
    match &info.seat {
        None => vec![],
        Some(seat) => vs(seat, loops, &xh(info, rot, n))
            .into_iter()
            .filter_map(vh)
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// eye loops (`av`/`cv`/`uv`/`Sl`/`Ml`/`lv`/`dv`/`fv`/`ah`/`Tl`/`ch`)
// ---------------------------------------------------------------------------

const OV: f64 = 0.26;
const SV: f64 = 0.57;
const IS: f64 = 0.64;
const WL: usize = 11;
const IV: usize = 18;
const AS2: usize = 48;
const BL: usize = 10;
const XL: usize = 64;

/// `cv`'s output: the loops plus the face's `shiftY` pitch.
pub(crate) struct EyeLoops {
    pub loops: Vec<Vec<V2>>,
    pub shift_y: f64,
}

struct AvEye {
    width: f64,
    height: f64,
    gap: f64,
    shift_y: f64,
}

/// `av` — per-body eye box metrics.
fn av(e: &str) -> Option<AvEye> {
    let (width, height, gap, shift_y) = match e {
        "cloud" => (0.21294, 0.46683, 0.470016, 0.07),
        "square" => (0.2553258426966292, 0.559752808988764, 0.529408, 0.12),
        "sparkle" => (0.18382, 0.40299, 0.41216, 0.0),
        "clover" => (0.23296, 0.51072, 0.559104, -0.02),
        "heartChubby" => (0.186576, 0.409032, 0.446208, 0.052),
        "starFlower" => (0.19292, 0.42294, 0.382976, 0.12),
        "teardrop" => (0.207792, 0.455544, 0.452608, 0.18),
        "tablet" => (0.2132, 0.4674, 0.587776, 0.02),
        "wedge" => (0.2132, 0.4674, 0.467712, 0.18),
        "house" => (0.2295124, 0.5031618, 0.512512, 0.18499534000463813),
        "star6" => (0.184782, 0.44965989, 0.41913344, 0.12),
        "pebble" => (0.2392, 0.5244, 0.546304, 0.12),
        "bean" => (0.2028, 0.4446, 0.4608, 0.12),
        "egg" => (0.2392, 0.5244, 0.433664, 0.11299906800092761),
        "squircle" => (0.2392, 0.5244, 0.5632, 0.12),
        "capsule" => (0.2106, 0.4617, 0.360448, 0.12),
        "cylinder" => (0.2392, 0.5244, 0.47872, 0.12),
        "hex" => (0.26, 0.57, 0.64, 0.12),
        "gem" => (0.2392, 0.5244, 0.501248, 0.12),
        "crystal" => (0.1612, 0.3534, 0.3712, 0.12),
        "shield" => (0.267904, 0.587328, 0.444928, 0.12),
        "dome" => (0.2132, 0.4674, 0.47872, 0.10599813600185524),
        "arch" => (0.1924, 0.4218, 0.448, 0.16),
        "leaf" => (0.1872, 0.4104, 0.448, 0.04),
        _ => return None,
    };
    Some(AvEye {
        width,
        height,
        gap,
        shift_y,
    })
}

/// `Oe` — a number with a fallback.
fn oe(e: f64, t: f64) -> f64 {
    if e.is_finite() {
        e
    } else {
        t
    }
}

/// `lc` — a rendered oval eye.
fn lc(e: &Eye) -> bool {
    e.oval[0] > 1e-4 && e.oval[1] > 1e-4
}

/// `Ml` — an eye's placement summary for the oval path.
struct Ml {
    on: bool,
    oval: bool,
    width: f64,
    height: f64,
    tilt: f64,
    cx: f64,
    cy: f64,
}

const VL: Ml = Ml {
    on: false,
    oval: false,
    width: 0.0,
    height: 0.0,
    tilt: 0.0,
    cx: 0.0,
    cy: 0.0,
};

fn ml(e: &Eye) -> Ml {
    let t = lc(e);
    if !(e.wid > 0.0) && !t {
        return VL;
    }
    // `e.lidY || 1` — 0 and NaN are falsy.
    let n = if e.lid_y == 0.0 || e.lid_y.is_nan() {
        1.0
    } else {
        e.lid_y
    };
    let r = (if e.tilt.is_finite() { e.tilt } else { 0.0 }) * (180.0 / std::f64::consts::PI);
    if t {
        return Ml {
            on: true,
            oval: true,
            width: 2.0 * e.oval[0],
            height: 2.0 * e.oval[1] * n,
            tilt: r,
            cx: e.off[0],
            cy: e.off[1],
        };
    }
    let o = &e.pts;
    if o.len() < 2 {
        return Ml {
            on: false,
            oval: false,
            width: 0.0,
            height: 0.0,
            tilt: 0.0,
            cx: e.off[0],
            cy: e.off[1],
        };
    }
    let s = o[0];
    let i = o[o.len() - 1];
    let a = i[0] - s[0];
    let c = i[1] - s[1];
    let u = a.hypot(c);
    let l = 2.0 * e.wid;
    Ml {
        on: true,
        oval: false,
        width: l,
        height: (u + l) * n,
        tilt: a.atan2(c) * (180.0 / std::f64::consts::PI) + r,
        cx: e.off[0] + (s[0] + i[0]) / 2.0,
        cy: e.off[1] + (s[1] + i[1]) / 2.0 * n,
    }
}

/// `Sl` — an oval eye loop (`XL` pts, tilted).
fn sl(e: f64, t: f64, n: f64, r: f64, o: f64) -> Vec<V2> {
    let s = dc(t, 0.01, 1.0) / 2.0;
    let i = dc(n.max(t), 0.02, 1.8) / 2.0;
    let a = -r * std::f64::consts::PI / 180.0;
    let c = a.cos();
    let u = a.sin();
    (0..XL)
        .map(|d| {
            let f = 2.0 * std::f64::consts::PI * d as f64 / XL as f64;
            let h = e * i * f.cos();
            let p = s * f.sin();
            [h * c - p * u + e * o / 2.0, h * u + p * c]
        })
        .collect()
}

/// `uv` — the all-oval fast path: two ellipse loops + `shiftY`.
fn uv(e: &Ml, t: &Ml, n: &AvEye, r: f64, o: f64, s: f64) -> EyeLoops {
    let i = oe(e.width * r, 0.0);
    let a = oe(e.height * r, 0.0);
    let c = oe(t.width * r, 0.0);
    let u = oe(t.height * r, 0.0);
    let l = oe(i.min(a), 0.0).max(0.0);
    let d = oe(i.max(a), 0.0).max(0.0);
    let f = oe(c.min(u), 0.0).max(0.0);
    let h = oe(c.max(u), 0.0).max(0.0);
    let p = oe(
        (IS + if e.on { oe(e.cx, 0.0) } else { 0.0 } + if t.on { oe(t.cx, 0.0) } else { 0.0 }) * s,
        IS * s,
    )
    .max(0.0);
    let g = dc(p, 0.05, 1.8).max(0.05);
    EyeLoops {
        loops: vec![
            sl(
                -1.0,
                l,
                d,
                -oe(oe(e.tilt, 0.0) + if i >= a { 0.0 } else { 90.0 }, 0.0),
                g,
            ),
            sl(
                1.0,
                f,
                h,
                oe(oe(t.tilt, 0.0) + if c >= u { 0.0 } else { 90.0 }, 0.0),
                g,
            ),
        ],
        shift_y: oe(
            n.shift_y + (oe(e.cy, 0.0) + oe(t.cy, 0.0)) / 2.0 * o,
            n.shift_y,
        ),
    }
}

/// `Al` — a unit vector.
fn al(e: f64, t: f64) -> V2 {
    let n = e.hypot(t);
    let n = if n == 0.0 { 1.0 } else { n };
    [e / n, t / n]
}

/// `Tl` — a round cap from `t` to `n` around `e`.
fn tl(e: V2, t: V2, n: V2, r: f64) -> Vec<V2> {
    let mut o = Vec::new();
    let s = (t[1] - e[1]).atan2(t[0] - e[0]);
    let mut a = (n[1] - e[1]).atan2(n[0] - e[0]) - s;
    while a > std::f64::consts::PI {
        a -= TAU;
    }
    while a < -std::f64::consts::PI {
        a += TAU;
    }
    let c = if a >= 0.0 { 1.0 } else { -1.0 };
    for u in 1..BL {
        let l = s + c * (u as f64 / BL as f64) * std::f64::consts::PI;
        o.push([e[0] + r * l.cos(), e[1] + r * l.sin()]);
    }
    o
}

/// `ah` — a stroke's outline: offset curve both ways + round caps.
fn ah(e: &[V2], t: &[f64], n: &[f64], r: f64, o: V2, s: V2) -> Vec<V2> {
    let i = e.len();
    let a: Vec<V2> = e
        .iter()
        .enumerate()
        .map(|(f, d)| [d[0] + t[f] * r, d[1] + n[f] * r])
        .collect();
    let c: Vec<V2> = e
        .iter()
        .enumerate()
        .map(|(f, d)| [d[0] - t[f] * r, d[1] - n[f] * r])
        .collect();
    let u = [e[0][0] - o[0] * r, e[0][1] - o[1] * r];
    let l = [e[i - 1][0] + s[0] * r, e[i - 1][1] + s[1] * r];
    let mut out = a;
    out.extend(tl(e[i - 1], out[out.len() - 1], l, r));
    out.extend(c.into_iter().rev());
    let last = out.len() - 1;
    out.extend(tl(e[0], out[last], u, r));
    out
}

/// `ch` — the fat-circle shortcut: when `t` exceeds the spine's extent the
/// loop is a disc.
fn ch(e: &[V2], t: f64) -> Option<Vec<V2>> {
    let (mut n, mut r) = (0.0, 0.0);
    for i in e {
        n += i[0];
        r += i[1];
    }
    n /= e.len() as f64;
    r /= e.len() as f64;
    let mut o = 0.0f64;
    for i in e {
        o = o.max((i[0] - n).hypot(i[1] - r));
    }
    if t <= o * 1.05 {
        return None;
    }
    let mut s = Vec::with_capacity(AS2);
    for i in 0..AS2 {
        let a = TAU * i as f64 / AS2 as f64;
        let c = a.cos();
        let u = a.sin();
        let (mut l, mut d, mut f) = (f64::NEG_INFINITY, 0.0, 0.0);
        for h in e {
            let p = h[0] * c + h[1] * u;
            if p > l {
                l = p;
                d = h[0];
                f = h[1];
            }
        }
        s.push([d + c * t, f + u * t]);
    }
    Some(s)
}

/// `dv` — the polyline stroke outline.
fn dv_eye(e: &[V2], t: f64) -> Vec<V2> {
    let mut n: Vec<V2> = Vec::new();
    for u in e {
        let l = n.last();
        if l.map_or(true, |l| (u[0] - l[0]).hypot(u[1] - l[1]) > 1e-6) {
            n.push(*u);
        }
    }
    if n.is_empty() || t <= 0.0 {
        return vec![];
    }
    if let Some(r) = ch(&n, t) {
        return r;
    }
    let o = n.len();
    if o < 2 {
        return vec![];
    }
    let mut s = vec![0.0; o];
    let mut i = vec![0.0; o];
    for u in 0..o {
        let l = n[u.saturating_sub(1)];
        let d = n[(u + 1).min(o - 1)];
        let f = (d[0] - l[0]).hypot(d[1] - l[1]);
        let f = if f == 0.0 { 1.0 } else { f };
        let mut h = -(d[1] - l[1]) / f;
        let mut p = (d[0] - l[0]) / f;
        if u > 0 && h * s[u - 1] + p * i[u - 1] < 0.0 {
            h = -h;
            p = -p;
        }
        s[u] = h;
        i[u] = p;
    }
    let a = al(n[1][0] - n[0][0], n[1][1] - n[0][1]);
    let c = al(
        n[o - 1][0] - n[o - 2][0],
        n[o - 1][1] - n[o - 2][1],
    );
    ah(&n, &s, &i, t, a, c)
}

/// `El`/`Pl` — anchor + control points of a `FB`-path segment.
fn el(e: &[f64; IV], t: usize) -> V2 {
    [e[t * 6], e[t * 6 + 1]]
}
fn pl(e: &[f64; IV], t: usize, n: usize) -> V2 {
    [e[t * 6] + e[t * 6 + n], e[t * 6 + 1] + e[t * 6 + n + 1]]
}

/// `hv`/`pv` — cubic position/tangent.
fn hv_c(e: V2, t: V2, n: V2, r: V2, o: f64) -> V2 {
    let s = 1.0 - o;
    [
        s * s * s * e[0] + 3.0 * s * s * o * t[0] + 3.0 * s * o * o * n[0] + o * o * o * r[0],
        s * s * s * e[1] + 3.0 * s * s * o * t[1] + 3.0 * s * o * o * n[1] + o * o * o * r[1],
    ]
}
fn pv_c(e: V2, t: V2, n: V2, r: V2, o: f64) -> V2 {
    let s = 1.0 - o;
    [
        3.0 * s * s * (t[0] - e[0]) + 6.0 * s * o * (n[0] - t[0]) + 3.0 * o * o * (r[0] - n[0]),
        3.0 * s * s * (t[1] - e[1]) + 6.0 * s * o * (n[1] - t[1]) + 3.0 * o * o * (r[1] - n[1]),
    ]
}

/// `fv` — the `FB` path stroke outline (tangent-continuous).
fn fv_eye(e: &[f64; IV], t: &impl Fn(f64, f64) -> V2, n: f64) -> Vec<V2> {
    if n <= 0.0 {
        return vec![];
    }
    let mut r: Vec<V2> = Vec::new();
    let mut tangents: Vec<V2> = Vec::new();
    for f in 0..2 {
        let h = el(e, f);
        let p = pl(e, f, 4);
        let g = pl(e, f + 1, 2);
        let m = el(e, f + 1);
        let y0 = if f == 0 { 0 } else { 1 };
        for y in y0..WL {
            let w = y as f64 / (WL - 1) as f64;
            let s = hv_c(h, p, g, m, w);
            let b = pv_c(h, p, g, m, w);
            let v = t(s[0], s[1]);
            let ee = t(s[0] + b[0], s[1] + b[1]);
            r.push(v);
            tangents.push([ee[0] - v[0], ee[1] - v[1]]);
        }
    }
    if let Some(o) = ch(&r, n) {
        return o;
    }
    let s = r.len();
    let mut i = vec![0.0; s];
    let mut a = vec![0.0; s];
    for f in 0..s {
        let h = tangents[f][0].hypot(tangents[f][1]);
        let h = if h == 0.0 { 1.0 } else { h };
        let mut p = -tangents[f][1] / h;
        let mut g = tangents[f][0] / h;
        if f > 0 && p * i[f - 1] + g * a[f - 1] < 0.0 {
            p = -p;
            g = -g;
        }
        i[f] = p;
        a[f] = g;
    }
    let c = r[0];
    let u = r[s - 1];
    let l = tangents[0][0].hypot(tangents[0][1]);
    let l = if l == 0.0 { 1.0 } else { l };
    let d = tangents[s - 1][0].hypot(tangents[s - 1][1]);
    let d = if d == 0.0 { 1.0 } else { d };
    let _ = (c, u);
    ah(
        &r,
        &i,
        &a,
        n,
        [tangents[0][0] / l, tangents[0][1] / l],
        [tangents[s - 1][0] / d, tangents[s - 1][1] / d],
    )
}

/// `lv` — the general path: stroke/polygon eye loops.
fn lv_eyes(e: &[Eye; 2], t: f64, n: f64, r: f64, o: f64) -> Vec<Vec<V2>> {
    let mut s: Vec<Vec<V2>> = Vec::new();
    let i = -o.sin();
    for (c, a) in e.iter().enumerate() {
        let u = lc(a);
        if !(a.wid > 0.0) && !u || !u && a.pts.len() < 2 {
            continue;
        }
        let l = if c == 0 { -1.0 } else { 1.0 };
        let d = oe(a.lid_y, 1.0);
        let d = if d == 0.0 { 1.0 } else { d };
        let f = (if a.tilt.is_finite() { a.tilt } else { 0.0 }).cos();
        let h = (if a.tilt.is_finite() { a.tilt } else { 0.0 }).sin();
        let p = l * ((IS / 2.0 + oe(a.off[0], 0.0)) * n);
        let g = i + oe(a.off[1], 0.0) * r;
        let m = |sv2: f64, b: f64| -> V2 {
            let v = sv2 * f - b * h;
            let ee = sv2 * h + b * f;
            [p + l * v * t, g + ee * t * d]
        };
        if let Some(fr) = &a.fill_ring {
            if fr.len() >= 3 {
                let sv2 = oe(a.off[0], 0.0);
                let b = oe(a.off[1], 0.0);
                s.push(fr.iter().map(|&[v, ee]| m(v - sv2, (ee - b) * d)).collect());
                continue;
            }
        }
        if u {
            let mut sv2: Vec<V2> = Vec::with_capacity(AS2);
            for b in 0..AS2 {
                let v = TAU * b as f64 / AS2 as f64;
                sv2.push(m(a.oval[0] * v.cos(), a.oval[1] * d * v.sin()));
            }
            s.push(sv2);
            continue;
        }
        let y = a.wid * t;
        let w = match &a.path {
            Some(p) => fv_eye(p, &m, y),
            None => dv_eye(
                &a.pts.iter().map(|&sv2| m(sv2[0], sv2[1])).collect::<Vec<_>>(),
                y,
            ),
        };
        if !w.is_empty() {
            s.push(w);
        }
    }
    s
}

/// `cv` — the body's eye loops at a face (oval fast path or general).
pub(crate) fn cv(e: &str, t: &[Eye; 2]) -> EyeLoops {
    let Some(n) = av(e) else {
        return EyeLoops {
            loops: vec![],
            shift_y: 0.0,
        };
    };
    let r = n.width / OV;
    let o = n.height / SV;
    let s = n.gap / IS;
    if t.iter()
        .all(|a| lc(a) && a.fill_ring.as_ref().map_or(true, |r| r.is_empty()))
    {
        let e0 = ml(&t[0]);
        let t0 = ml(&t[1]);
        uv(&e0, &t0, &n, r, o, s)
    } else {
        EyeLoops {
            loops: lv_eyes(t, r, s, o, n.shift_y),
            shift_y: 0.0,
        }
    }
}

/// `gc` — loops as `M..Z` path data.
pub(crate) fn gc(e: &[Vec<V2>]) -> String {
    let mut t = String::new();
    for n in e {
        if n.is_empty() {
            continue;
        }
        t += &format!("M{} {}", un(n[0][0]), un(n[0][1]));
        for r in 1..n.len() {
            t += &format!(" {} {}", un(n[r][0]), un(n[r][1]));
        }
        t += "Z";
    }
    t
}

// ---------------------------------------------------------------------------
// the atlas (`decodeAtlas`/`atlasOutline`, `im` standing in for `IM`+`Wn`)
// ---------------------------------------------------------------------------

/// A decoded atlas (`decodeAtlas`'s record).
pub(crate) struct Atlas {
    pub mirror: bool,
    pub m: f64,
    pub c: f64,
    pub kf: f64,
    pub step: f64,
    pub k: usize,
    pub bytes: Vec<u8>,
}

/// `atob` — base64 decode (the `A-Za-z0-9+/` alphabet, `=` padding).
fn b64(e: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(e.len() * 3 / 4);
    let mut acc = 0u32;
    let mut nbits = 0u32;
    for c in e.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => continue,
        } as u32;
        acc = (acc << 6) | v;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((acc >> nbits) as u8);
        }
    }
    out
}

fn decode_atlas(entry: &AtlasEntry, step: f64, k: usize) -> Atlas {
    Atlas {
        mirror: entry.mirror,
        m: entry.m,
        c: entry.c,
        kf: entry.kf,
        step,
        k,
        bytes: b64(entry.r),
    }
}

/// The decoded atlas for a body (`atlases` cache, `null` when none).
fn atlas_of(body: &'static str) -> Option<Rc<Atlas>> {
    ATLAS_CACHE.with(|c| {
        if let Some(a) = c.borrow().get(body) {
            return a.clone();
        }
        let a = ATLAS
            .iter()
            .find(|(n, _)| *n == body)
            .map(|(_, e)| Rc::new(decode_atlas(e, ATLAS_STEP, ATLAS_K)));
        c.borrow_mut().insert(body, a.clone());
        a
    })
}

/// `rows` — `yS`'s rotation at roll 0 as rows (x,y,view axes).
fn rows(yaw: f64, pitch: f64) -> [f64; 9] {
    let cy = yaw.cos();
    let sy = yaw.sin();
    let cp = pitch.cos();
    let sp = pitch.sin();
    [
        cy,
        0.0,
        sy,
        sy * sp,
        cp,
        -cy * sp,
        -sy * cp,
        sp,
        cy * cp,
    ]
}

/// `atlasOutline` — `IM(body, rot, n)`'s outline as `gc` path data.
fn atlas_outline(a: &Atlas, n: f64, yaw: f64, pitch: f64, roll: f64) -> String {
    let deg = std::f64::consts::PI / 180.0;
    let (step, kk) = (a.step, a.k);
    let j = 360.0 / step;
    let p = 180.0 / step + 1.0;
    let cols = if a.mirror { 180.0 / step + 1.0 } else { j } as usize;
    let q = rows(yaw * deg, pitch * deg);
    let u = (((yaw + 180.0) / step) % j + j) % j;
    let j0 = (u.floor() as usize) % (j as usize);
    let ty = u - u.floor();
    let v = dc((pitch + 90.0) / step, 0.0, p - 1.0);
    let k0 = (p - 2.0).min(v.floor()) as usize;
    let tp = v - k0 as f64;
    let mut radii = vec![0.0f64; kk];
    let (mut cx, mut cy) = (0.0f64, 0.0f64);
    let nodes = [
        [j0 as f64, k0 as f64, (1.0 - ty) * (1.0 - tp)],
        [((j0 + 1) % (j as usize)) as f64, k0 as f64, ty * (1.0 - tp)],
        [j0 as f64, (k0 + 1) as f64, (1.0 - ty) * tp],
        [((j0 + 1) % (j as usize)) as f64, (k0 + 1) as f64, ty * tp],
    ];
    for &[jj, row, w] in &nodes {
        if w <= 0.0 {
            continue;
        }
        let node_yaw = -180.0 + jj * step;
        let r = rows(node_yaw * deg, (-90.0 + row * step) * deg);
        let dot = q[0] * r[6] + q[1] * r[7] + q[2] * r[8];
        let ax = q[0] - dot * r[6];
        let ay = q[1] - dot * r[7];
        let az = q[2] - dot * r[8];
        let phi = (ax * r[3] + ay * r[4] + az * r[5])
            .atan2(ax * r[0] + ay * r[1] + az * r[2]);
        let flip = a.mirror && node_yaw < 0.0;
        let base = (row as usize * cols
            + if a.mirror {
                (node_yaw.abs() / step).round() as usize
            } else {
                jj as usize
            })
            * (kk + 2);
        let ncx = (a.bytes[base] as f64 / 255.0 * 2.0 - 1.0) * a.c
            * if flip { -1.0 } else { 1.0 };
        let ncy = (a.bytes[base + 1] as f64 / 255.0 * 2.0 - 1.0) * a.c;
        cx += w * (ncx * phi.cos() + ncy * phi.sin());
        cy += w * (ncy * phi.cos() - ncx * phi.sin());
        for i in 0..kk {
            let mut t = TAU * i as f64 / kk as f64 + phi;
            if flip {
                t = std::f64::consts::PI - t;
            }
            let mut f = ((t / TAU) * kk as f64) % kk as f64;
            if f < 0.0 {
                f += kk as f64;
            }
            let i0 = (f.floor() as usize) % kk;
            let fr = f - f.floor();
            radii[i] += w * (a.bytes[base + 2 + i0] as f64 * (1.0 - fr)
                + a.bytes[base + 2 + (i0 + 1) % kk] as f64 * fr);
        }
    }
    let c = n / 2.0;
    let o = a.kf * n;
    let s = a.m / 255.0;
    let turn = roll * deg;
    let ct = turn.cos();
    let st = turn.sin();
    let mut d = String::new();
    for i in 0..kk {
        let t = TAU * i as f64 / kk as f64;
        let px = cx + s * radii[i] * t.cos();
        let py = cy + s * radii[i] * t.sin();
        let x = ((c + o * (px * ct - py * st)) * 100.0).round() / 100.0;
        let y = ((c - o * (py * ct + px * st)) * 100.0).round() / 100.0;
        d += &format!("{}{} {}", if i == 0 { "M" } else { " " }, x, y);
    }
    d + "Z"
}

/// `im` — `IM`: the body's sprite outline(s) at a pose. The shell patches
/// `Wn` so an atlased body never solves; a body with no atlas has no
/// reachable caller, so it returns the empty list (the JS fallback solved
/// silhouettes live — dropped, see the module header).
pub(crate) fn im(body: &'static str, rot: &Rot, n: f64) -> Vec<String> {
    match atlas_of(body) {
        Some(a) => vec![atlas_outline(&a, n, rot.yaw, rot.pitch, rot.roll)],
        None => vec![],
    }
}

/// `$h`'s output: the body sprite path(s) + the eye paths.
pub(crate) struct ShOut {
    pub body: Vec<String>,
    pub eyes: Vec<String>,
}

/// `$h` — the raster step of `EM`/`PM`: sprite-space body + eye paths.
pub(crate) fn dollar_h(body: &'static str, rot: &Rot, n: f64, eyes: Option<&[Eye; 2]>) -> ShOut {
    let o = match eyes {
        Some(r) => {
            let info = cs(body);
            match info {
                Some(info) => gs(&*info, rot, n, &cv(body, r)),
                None => vec![],
            }
        }
        None => vec![],
    };
    ShOut {
        body: im(body, rot, n),
        eyes: o.iter().map(|s| gc(std::slice::from_ref(s))).collect(),
    }
}
