//! The mark motion engine — `mark/motion.mjs` ported.
//!
//! `ih` is [`Mark`] (the clip player: springs, tracks, gaze, pokes, face),
//! `rv` is [`Director`] (the state table `yl`), `Ix` is [`MorphFx`] (the
//! status-effect engine behind `setMorph`), `ax` is [`Face`] (the
//! expression machine). Rendering lives in `render` (`Uh`), body geometry
//! in `bodies`, rasterization in `paint`; `clips`, `atlas_data` and
//! `rest_data` are the baked data of the TS originals.
//!
//! Skipped from the source: the proof-lighting render (`isProof` is never
//! set by the avatar) and the `IM()` silhouette solver + polygon union —
//! outlines come from the pre-solved atlas, as `blob.tsx` arranges by
//! overriding `Wn`.

mod atlas_data;
mod bodies;
#[cfg(test)]
mod brand;
mod clips;
mod paint;
mod render;
mod rest_data;

use std::collections::HashMap;

// `BodyInfo` is re-exported for the avatar/picker surface that lands with
// the detail pane — nothing consumes it yet.
#[allow(unused_imports)]
pub(crate) use bodies::BodyInfo;
pub(crate) use paint::{
    draw_fx, gaze_advance, gaze_set, paint_rest, paint_tree, rest_bytes, GazeState,
};
pub(crate) use render::{uh, MarkTree, PaintOpts, UhSize};

#[allow(unused_imports)]
pub(crate) use clips::STATES;

pub(crate) type V2 = [f64; 2];
pub(crate) type V3 = [f64; 3];
pub(crate) type V4 = [f64; 4];
pub(crate) type M3 = [f64; 9];

// ---------------------------------------------------------------------------
// small math (motion.mjs's helpers, same names where it helps review)
// ---------------------------------------------------------------------------

const TAU: f64 = std::f64::consts::TAU;
pub(crate) const MN: f64 = std::f64::consts::PI / 180.0;

#[inline]
pub(crate) fn le(e: f64, t: f64, n: f64) -> f64 {
    n.min(t.max(e))
}
#[inline]
pub(crate) fn v2_lerp(e: V2, t: V2, n: f64) -> V2 {
    [e[0] + (t[0] - e[0]) * n, e[1] + (t[1] - e[1]) * n]
}
pub(crate) fn hypot2(a: f64, b: f64) -> f64 {
    (a * a + b * b).sqrt()
}
pub(crate) fn hypot3(a: f64, b: f64, c: f64) -> f64 {
    (a * a + b * b + c * c).sqrt()
}

/// `ts` — cubic in-out.
pub(crate) fn ts(e: f64) -> f64 {
    if e < 0.5 { 4.0 * e * e * e } else { 1.0 - (-2.0 * e + 2.0).powi(3) / 2.0 }
}
/// `Ye` — cubic out.
pub(crate) fn ye(e: f64) -> f64 {
    1.0 - (1.0 - e).powi(3)
}
/// `wb` — smoothstep.
pub(crate) fn wb(e: f64) -> f64 {
    e * e * (3.0 - 2.0 * e)
}
const RR: [f64; 4] = [0.42, 0.0, 0.58, 1.0];
const BB: [f64; 4] = [0.4, 0.06, 0.18, 1.0];
const XB: f64 = 0.28;
const IA: f64 = 0.35;
const VB: f64 = 4.25 / 60.0;
const SB: f64 = 2.0 / 60.0;
const UI: f64 = 1.06;

/// `rc` — a CSS cubic-bezier ease at `e`.
fn rc(e: f64, t: [f64; 4]) -> f64 {
    let [n, r, o, s] = t;
    let mut i = e;
    for _ in 0..8 {
        let c = sl(i, n, o) - e;
        let u = mb(i, n, o);
        if c.abs() < 1e-6 || u.abs() < 1e-6 {
            break;
        }
        i = (i - c / u).clamp(0.0, 1.0);
    }
    sl(i, r, s)
}
fn sl(e: f64, t: f64, n: f64) -> f64 {
    let r = 1.0 - e;
    3.0 * r * r * e * t + 3.0 * r * e * e * n + e * e * e
}
fn mb(e: f64, t: f64, n: f64) -> f64 {
    let r = 1.0 - e;
    3.0 * r * r * t + 6.0 * r * e * (n - t) + 3.0 * e * e * (1.0 - n)
}
/// `ns` — clamped smoothstep.
fn ns(e: f64) -> f64 {
    let t = e.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
/// `li` — the blink window `{u1,u2}` for a duration.
fn li(e: bool, t: f64) -> (f64, f64) {
    let r = (if e { SB } else { VB } / t.max(0.05)).min(0.28);
    let o = (1.0 - r) * 0.42;
    (o, o + r)
}
/// `Tb` — the reopen curve.
fn tb(e: f64) -> f64 {
    let t = e.clamp(0.0, 1.0);
    if t < 0.7 {
        let n = t / 0.7;
        (1.0 - (1.0 - n) * (1.0 - n)) * UI
    } else {
        UI + (1.0 - UI) * ns((t - 0.7) / 0.3)
    }
}

// ---------------------------------------------------------------------------
// springs — `Ao`/`Io`
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Spring {
    x: f64,
    v: f64,
    t: f64,
}
fn ao(e: f64) -> Spring {
    Spring { x: e, v: 0.0, t: e }
}
fn io(e: &mut Spring, t: f64, n: f64, r: f64) {
    e.v += (-2.0 * n * t * e.v - t * t * (e.x - e.t)) * r;
    e.x += e.v * r;
    if !e.x.is_finite() || !e.v.is_finite() {
        e.x = e.t;
        e.v = 0.0;
    }
}
const KB: f64 = 1.0 / 120.0;
fn qf(e: f64) -> usize {
    (e / KB).ceil().max(1.0) as usize
}

// ---------------------------------------------------------------------------
// quaternions / rotations — `ca`…`Et`
// ---------------------------------------------------------------------------

pub(crate) fn ca(e: V4, t: V4) -> V4 {
    [
        e[0] * t[0] - e[1] * t[1] - e[2] * t[2] - e[3] * t[3],
        e[0] * t[1] + e[1] * t[0] + e[2] * t[3] - e[3] * t[2],
        e[0] * t[2] - e[1] * t[3] + e[2] * t[0] + e[3] * t[1],
        e[0] * t[3] + e[1] * t[2] - e[2] * t[1] + e[3] * t[0],
    ]
}
pub(crate) fn axq(e: V3, t: f64) -> V4 {
    let n = (t / 2.0).sin();
    [(t / 2.0).cos(), e[0] * n, e[1] * n, e[2] * n]
}
fn pb(e: V4) -> V4 {
    let t = hypot3(e[0], e[1], e[2]).max(f64::EPSILON);
    [e[0] / t, e[1] / t, e[2] / t, e[3] / t]
}
pub(crate) fn ab(e: V3, t: V3) -> V4 {
    let n = 1.0 + (e[0] * t[0] + e[1] * t[1] + e[2] * t[2]);
    pb([
        n,
        e[1] * t[2] - e[2] * t[1],
        e[2] * t[0] - e[0] * t[2],
        e[0] * t[1] - e[1] * t[0],
    ])
}
pub(crate) fn cr(e: V4) -> M3 {
    let [t, n, r, o] = e;
    [
        1.0 - 2.0 * (r * r + o * o),
        2.0 * (n * r - t * o),
        2.0 * (n * o + t * r),
        2.0 * (n * r + t * o),
        1.0 - 2.0 * (n * n + o * o),
        2.0 * (r * o - t * n),
        2.0 * (n * o - t * r),
        2.0 * (r * o + t * n),
        1.0 - 2.0 * (n * n + r * r),
    ]
}
pub(crate) fn ib(e: f64, t: f64, n: f64) -> V4 {
    ca(axq([0.0, 0.0, 1.0], n), ca(axq([0.0, 1.0, 0.0], t), axq([1.0, 0.0, 0.0], e)))
}
pub(crate) fn jf(e: M3) -> V3 {
    let (t, n, r, o, s, i, a, _c) = (e[0], e[3], e[5], e[4], e[6], e[7], e[8], e[2]);
    let c2 = (-s).clamp(-1.0, 1.0);
    let u = c2.asin();
    if c2.abs() > 0.999999 {
        [(-r).atan2(o), u, 0.0]
    } else {
        [i.atan2(a), u, n.atan2(t)]
    }
}
pub(crate) fn q_to_euler(e: V4) -> V3 {
    jf(cr(e))
}
pub(crate) fn et(e: M3, t: V3) -> V3 {
    [
        e[0] * t[0] + e[1] * t[1] + e[2] * t[2],
        e[3] * t[0] + e[4] * t[1] + e[5] * t[2],
        e[6] * t[0] + e[7] * t[1] + e[8] * t[2],
    ]
}

// ---------------------------------------------------------------------------
// eyes — the expression table `vn` and its builders
// ---------------------------------------------------------------------------

/// `oc` — the two eye capsules (position A→B, axis N) on the sphere.
const OC: [(V3, V3, V3); 2] = [
    ([0.01722, 0.45638, 0.88962], [0.186, 0.20949, 0.95996], [0.82924, 0.4906, -0.26773]),
    ([0.48863, 0.67924, 0.54762], [0.65674, 0.43195, 0.61815], [0.60389, 0.18972, -0.77416]),
];

/// An eye's frame on the face (`eh`'s `{M,T,C}` per eye).
#[derive(Clone, Copy)]
pub(crate) struct EyeFrame {
    pub m: V3,
    pub t: V3,
    pub c: V3,
}

/// Constants `ih`/`Uh` need that JS computed at module load: the eye frames
/// (`eh`, `FT`), the rest pose rotation (`oh`) and the pencil trail (`vE`).
/// Built once per avatar — a few dozen flops, no shared cache needed.
pub(crate) struct Tables {
    pub eye_frames: [EyeFrame; 2],
    /// `FT`: each eye's winding flip.
    pub eye_flip: [bool; 2],
    /// `oh` — the rest pose euler (degrees), `hl`'s rotation track.
    pub rest_rot: V3,
    /// `vE` — the pencil scribble's station trail of `{at,point}`.
    pub pencil_trail: Vec<(f64, V2)>,
    /// `dE` — the teardrop ring the pencil morph travels (`lE(Sd, 48)`).
    pub pencil_ring: Vec<V2>,
    /// `Hh` — the circle ring every other morph travels.
    pub circle_ring: Vec<V2>,
    /// `pE` / `mE` — the pencil and bang shape paths.
    pub pencil_shape: String,
    pub bang_shape: String,
}

thread_local! {
    static TABLES: std::rc::Rc<Tables> = std::rc::Rc::new(tables());
}

/// [`tables`], built once and shared by every mark (each avatar mount
/// rebuilt them).
pub(crate) fn shared_tables() -> std::rc::Rc<Tables> {
    TABLES.with(std::rc::Rc::clone)
}

pub(crate) fn tables() -> Tables {
    // `Cb`/`Sr`/`Rb`/`eh`/`$b`/`oh` verbatim.
    let norm = |e: V3| -> V3 {
        let t = hypot3(e[0], e[1], e[2]).max(f64::EPSILON);
        [e[0] / t, e[1] / t, e[2] / t]
    };
    let mut cb = [0.0; 3];
    for (a, b, _) in OC {
        let r = norm([a[0] + b[0], a[1] + b[1], a[2] + b[2]]);
        cb = [cb[0] + r[0], cb[1] + r[1], cb[2] + r[2]];
    }
    let cb = norm(cb);
    // `Sr`: swing `Cb` to +Z, then roll so the eye axis lands horizontally.
    let mut e = ab(cb, [0.0, 0.0, 1.0]);
    let t = cr(e);
    let (mut n, mut r) = (0.0f64, 0.0f64);
    for (a, b, _) in OC {
        let c = et(t, [b[0] - a[0], b[1] - a[1], b[2] - a[2]]);
        n += c[0];
        r += c[1];
    }
    let o = r.atan2(n);
    let s = (((std::f64::consts::PI / 2.0 - o + std::f64::consts::PI / 2.0) % std::f64::consts::PI)
        + std::f64::consts::PI)
        % std::f64::consts::PI
        - std::f64::consts::PI / 2.0;
    e = ca(axq([0.0, 0.0, 1.0], s), e);
    let rb_q = e;
    let rb_m = cr(rb_q);
    let rb: [(V3, V3, V3); 2] = OC.map(|(a, b, nn)| (et(rb_m, a), et(rb_m, b), et(rb_m, nn)));
    // `eh`
    let eye_of = |(a, b): (V3, V3)| -> EyeFrame {
        let s = norm([a[0] + b[0], a[1] + b[1], a[2] + b[2]]);
        let i = b[0] * s[0] + b[1] * s[1] + b[2] * s[2];
        let mut tv = norm([b[0] - i * s[0], b[1] - i * s[1], b[2] - i * s[2]]);
        if tv[1] < 0.0 {
            tv = [-tv[0], -tv[1], -tv[2]];
        }
        let mut c = [
            s[1] * tv[2] - s[2] * tv[1],
            s[2] * tv[0] - s[0] * tv[2],
            s[0] * tv[1] - s[1] * tv[0],
        ];
        if c[0] * s[0] < 0.0 {
            c = [-c[0], -c[1], -c[2]];
        }
        EyeFrame { m: s, t: tv, c }
    };
    let eye_frames = [eye_of((rb[0].0, rb[0].1)), eye_of((rb[1].0, rb[1].1))];
    let eye_flip = eye_frames.map(|f| {
        let t = f.t;
        let c = f.c;
        let r = [
            c[1] * t[2] - c[2] * t[1],
            c[2] * t[0] - c[0] * t[2],
            c[0] * t[1] - c[1] * t[0],
        ];
        r[0] * f.m[0] + r[1] * f.m[1] + r[2] * f.m[2] < 0.0
    });
    // `$b`/`oh` — the rest pose.
    let sb = [rb_q[0], -rb_q[1], -rb_q[2], -rb_q[3]];
    let ee = q_to_euler(sb);
    let rest_rot = [ee[0] / MN, ee[1] / MN, ee[2] / MN];

    // `vE` — the pencil trail: sampled along `wc`'s path while the pencil
    // travels left→right (the Oo window).
    let oo = render::PS * render::PR;
    let mut trail: Vec<(f64, V2)> = Vec::new();
    for t in 0..(oo as usize) {
        let n = render::wh(t as f64);
        if trail.last().is_none_or(|&(_, p)| hypot2(n[0] - p[0], n[1] - p[1]) > render::SE) {
            trail.push((t as f64, n));
        }
    }
    // `Sd`/`dE`/`Ai` — the pencil's ring is the teardrop belt rotated half.
    let disc = bodies::fh(88.0, 18.0).ring;
    let pencil_ring = render::le_ring(&disc, disc.len() / 2);
    let circle_ring = (0..96)
        .map(|n| {
            let t = n as f64 / 96.0 * TAU;
            [bodies::X + t.cos() * bodies::X, bodies::X + t.sin() * bodies::X]
        })
        .collect();

    Tables {
        eye_frames,
        eye_flip,
        rest_rot,
        pencil_trail: trail,
        pencil_ring,
        circle_ring,
        pencil_shape: render::f_e(30.0, 88.0),
        bang_shape: render::h_e(30.0, 17.0, 96.0),
    }
}

// ---------------------------------------------------------------------------
// eye shape — `vn` and the morphs between eyes
// ---------------------------------------------------------------------------

const MR: f64 = 0.26;
const NB: usize = 21;
pub(crate) const FB: usize = 18;
const IL: f64 = 0.1305;

/// One eye of a frame (`_r` output, also the `be()` element).
#[derive(Clone)]
pub(crate) struct Eye {
    pub off: V2,
    pub tilt: f64,
    pub wid: f64,
    pub lid_y: f64,
    pub oval: V2,
    pub oval_mix: f64,
    pub pts: Vec<V2>,
    pub path: Option<[f64; FB]>,
    pub fill_ring: Option<Vec<V2>>,
}

impl Eye {
    /// `wt` — a copy with the oval blend reset.
    fn wt(&self) -> Eye {
        let mut e = self.clone();
        e.oval_mix = 0.0;
        e
    }
    /// `jt` — a rendered oval (not a bare capsule spec).
    fn is_oval(&self) -> bool {
        self.oval[0] > 1e-4 && self.oval[1] > 1e-4
    }
    /// `os` — the spine bows enough to keep it a path, not a stadium.
    fn is_bowed(&self) -> bool {
        if self.is_oval() || self.pts.len() < 3 {
            return false;
        }
        let t = self.pts[0];
        let n = self.pts[self.pts.len() - 1];
        let (r, o) = (n[0] - t[0], n[1] - t[1]);
        let s = r * r + o * o;
        if s < 1e-8 {
            return false;
        }
        let mut i = 0.0f64;
        for &a in &self.pts {
            let c = ((a[0] - t[0]) * r + (a[1] - t[1]) * o) / s;
            let (u, l) = (t[0] + c * r, t[1] + c * o);
            let d = hypot2(a[0] - u, a[1] - l);
            if d > i {
                i = d;
            }
        }
        i > 0.012f64.max(self.wid * 0.2)
    }
}

fn ho(e: &[f64; FB], t: usize) -> V2 {
    [e[t * 6], e[t * 6 + 1]]
}
fn po(e: &[f64; FB], t: usize, n: bool) -> V2 {
    let r = if n { 4 } else { 2 };
    [e[t * 6] + e[t * 6 + r], e[t * 6 + 1] + e[t * 6 + r + 1]]
}
/// `th` — a spine's sampled polyline (NB points over two cubics).
fn th(e: &[f64; FB]) -> Vec<V2> {
    let t = NB.div_ceil(2);
    let mut n = Vec::new();
    let r = |o: V2, s: V2, i: V2, a: V2, c: usize, n: &mut Vec<V2>| {
        for u in c..t {
            let l = u as f64 / (t - 1) as f64;
            let d = 1.0 - l;
            let (f, h, p, g) = (d * d * d, 3.0 * d * d * l, 3.0 * d * l * l, l * l * l);
            n.push([
                f * o[0] + h * s[0] + p * i[0] + g * a[0],
                f * o[1] + h * s[1] + p * i[1] + g * a[1],
            ]);
        }
    };
    r(ho(e, 0), po(e, 0, true), po(e, 1, false), ho(e, 1), 0, &mut n);
    r(ho(e, 1), po(e, 1, true), po(e, 2, false), ho(e, 2), 1, &mut n);
    n
}
/// `jb` — three points (+optional arc flag) → the FB-length spine array.
fn jb(p0: V2, p1: V2, p2: V2, is_arc: bool) -> [f64; FB] {
    let [o, s] = p0;
    let [i, a] = p1;
    let [c, u] = p2;
    let mut l = [0.0; FB];
    fn d(l: &mut [f64; FB], y: usize, w: V2) {
        l[y * 6] = w[0];
        l[y * 6 + 1] = w[1];
    }
    fn f(l: &mut [f64; FB], y: usize, w: V2) {
        l[y * 6 + 2] = w[0] - l[y * 6];
        l[y * 6 + 3] = w[1] - l[y * 6 + 1];
    }
    fn h(l: &mut [f64; FB], y: usize, w: V2) {
        l[y * 6 + 4] = w[0] - l[y * 6];
        l[y * 6 + 5] = w[1] - l[y * 6 + 1];
    }
    if is_arc {
        let y = 2.0 * (o * (a - u) + i * (u - s) + c * (s - a));
        if y.abs() > 1e-9 {
            let ss = o * o + s * s;
            let b = i * i + a * a;
            let v = c * c + u * u;
            let e = (ss * (a - u) + b * (u - s) + v * (s - a)) / y;
            let aa = (ss * (c - i) + b * (o - c) + v * (i - o)) / y;
            let f_r = hypot2(o - e, s - aa);
            let n = (s - aa).atan2(o - e);
            let r = (u - aa).atan2(c - e);
            let bb = (a - aa).atan2(i - e);
            let p = std::f64::consts::PI * 2.0;
            let i2 = (r - n + p) % p;
            let oo = if (bb - n + p) % p <= i2 { i2 } else { i2 - p };
            let hh = |k: f64| [e + f_r * k.cos(), aa + f_r * k.sin()];
            let lt = |k: f64| [-k.sin(), k.cos()];
            let j = oo / 2.0;
            let jd = 4.0 / 3.0 * (j / 4.0).tan() * f_r;
            let fe = [n, n + j, n + oo];
            for (k, &angle) in fe.iter().enumerate() {
                let te = hh(angle);
                let z = lt(angle);
                d(&mut l, k, te);
                if k > 0 {
                    f(&mut l, k, [te[0] - jd * z[0], te[1] - jd * z[1]]);
                }
                if k < 2 {
                    h(&mut l, k, [te[0] + jd * z[0], te[1] + jd * z[1]]);
                }
            }
            return l;
        }
        let w = [(o + c) / 2.0, (s + u) / 2.0];
        d(&mut l, 0, [o, s]);
        d(&mut l, 1, w);
        d(&mut l, 2, [c, u]);
        h(&mut l, 0, [o + (w[0] - o) / 3.0, s + (w[1] - s) / 3.0]);
        f(&mut l, 1, [w[0] - (w[0] - o) / 3.0, w[1] - (w[1] - s) / 3.0]);
        h(&mut l, 1, [w[0] + (c - w[0]) / 3.0, w[1] + (u - w[1]) / 3.0]);
        f(&mut l, 2, [c - (c - w[0]) / 3.0, u - (u - w[1]) / 3.0]);
        return l;
    }
    let p = [(o + i) / 2.0, (s + a) / 2.0];
    let g = [(i + c) / 2.0, (a + u) / 2.0];
    let m = [(p[0] + g[0]) / 2.0, (p[1] + g[1]) / 2.0];
    d(&mut l, 0, [o, s]);
    d(&mut l, 1, m);
    d(&mut l, 2, [c, u]);
    h(&mut l, 0, [o + 2.0 / 3.0 * (p[0] - o), s + 2.0 / 3.0 * (p[1] - s)]);
    f(&mut l, 1, [m[0] + 2.0 / 3.0 * (p[0] - m[0]), m[1] + 2.0 / 3.0 * (p[1] - m[1])]);
    h(&mut l, 1, [m[0] + 2.0 / 3.0 * (g[0] - m[0]), m[1] + 2.0 / 3.0 * (g[1] - m[1])]);
    f(&mut l, 2, [c + 2.0 / 3.0 * (g[0] - c), u + 2.0 / 3.0 * (g[1] - u)]);
    l
}
/// `_r` — an eye record; `lid` is the lid's three control points (`jb`).
fn eye(off: V2, tilt: f64, wid: f64, lid: [V2; 3], is_arc: bool, oval: V2) -> Eye {
    let s = jb(lid[0], lid[1], lid[2], is_arc);
    Eye {
        off,
        tilt,
        wid,
        lid_y: 1.0,
        oval,
        oval_mix: 0.0,
        pts: th(&s),
        path: Some(s),
        fill_ring: None,
    }
}
/// `sc` — a stadium eye.
fn sc(e: f64, t: f64, n: f64, r: V2, o: f64) -> Eye {
    let s = 0.0f64.max((t - e) / 2.0);
    let i = n * std::f64::consts::PI / 180.0;
    let (a, c) = (i.sin() * s, i.cos() * s);
    eye(r, o, e / 2.0, [[-a, -c], [0.0, 0.0], [a, c]], false, [0.0, 0.0])
}
/// `zn` — an open eye.
fn zn(e: f64, t: f64, n: f64, r: f64) -> Eye {
    sc(e, t, n, [r, 0.0], 0.0)
}
/// `qb` — a flat lid eye.
fn qb(e: f64, t: f64, n: f64) -> Eye {
    eye([n, 0.0], 0.0, t, [[-e, 0.0], [0.0, 0.0], [e, 0.0]], false, [e, t])
}
fn zt(e: Eye) -> [Eye; 2] {
    [e.clone(), e]
}

/// `vn` — the expression table (built once per `Tables` user via `Face::new`
/// calling this; entries are immutable shapes).
fn vn(name: &str) -> [Eye; 2] {
    match name {
        "neutral" => zt(zn(MR, 0.57, 0.0, 0.0)),
        "happy" => zt(eye(
            [0.03, 0.03],
            0.0,
            IL,
            [[0.16, -0.04], [0.0, 0.12], [-0.16, -0.04]],
            true,
            [0.0, 0.0],
        )),
        "surprised" => zt(zn(0.42, 0.86, 0.0, 0.03)),
        "inspecting" => [zn(0.54, 0.54, 0.0, 0.04), zn(0.36, 0.36, 0.0, 0.04)],
        "sad" => zt(zn(MR, 0.5, -28.0, -0.05)),
        "sleepy" => zt(eye(
            [0.01, 0.0],
            0.0,
            MR / 2.0,
            [[-0.11, 0.025], [0.0, -0.05], [0.11, 0.025]],
            false,
            [0.0, 0.0],
        )),
        "squinted" => zt(zn(MR, 0.5, 90.0, 0.01)),
        "calm" => zt(qb(0.296, 0.247, 0.05)),
        "confused" => zt(eye(
            [-0.1, 0.0],
            0.0,
            IL,
            [[0.01, 0.16], [-0.17, 0.0], [0.01, -0.16]],
            false,
            [0.0, 0.0],
        )),
        _ => zt(zn(MR, 0.57, 0.0, 0.0)),
    }
}

/// `Db`/`Hb` — alias an expression name into the table.
fn hb(e: &str) -> &'static str {
    let t = e.trim().to_lowercase();
    let n = match t.as_str() {
        "thinking" => "sad",
        "strange" => "inspecting",
        "angry" => "neutral",
        "inspect" => "inspecting",
        "squint" => "squinted",
        "idle" => "neutral",
        "smile" => "happy",
        "sleep" => "sleepy",
        _ => t.as_str(),
    };
    match n {
        "neutral" => "neutral",
        "happy" => "happy",
        "surprised" => "surprised",
        "sad" => "sad",
        "sleepy" => "sleepy",
        "squinted" => "squinted",
        "calm" => "calm",
        "inspecting" => "inspecting",
        "confused" => "confused",
        _ => "neutral",
    }
}
/// `Dr` — expressions that morph through a blink.
fn dr(e: &str) -> bool {
    matches!(e, "sad" | "happy" | "confused" | "squinted" | "sleepy")
}
/// `Vb` — faces that may blink.
fn vb(e: &str) -> bool {
    !dr(e) && !vn(e).iter().any(Eye::is_oval)
}

fn al(e: &[V2], t: usize, n: usize) -> V2 {
    if e.is_empty() {
        return [0.0, 0.0];
    }
    if e.len() == 1 || n <= 1 {
        return e[0];
    }
    let r = t as f64 / (n - 1) as f64 * (e.len() - 1) as f64;
    let o = (r.floor() as usize).min(e.len() - 2);
    v2_lerp(e[o], e[o + 1], r - o as f64)
}
/// `Xb` — spine morph.
fn xb(e: &Eye, t: &Eye, n: f64) -> Eye {
    let mut r = Eye {
        off: v2_lerp(e.off, t.off, n),
        tilt: e.tilt + (t.tilt - e.tilt) * n,
        wid: e.wid + (t.wid - e.wid) * n,
        lid_y: e.lid_y + (t.lid_y - e.lid_y) * n,
        oval: v2_lerp(e.oval, t.oval, n),
        oval_mix: 0.0,
        pts: Vec::new(),
        path: None,
        fill_ring: None,
    };
    if let (Some(ep), Some(tp)) = (e.path, t.path) {
        let a: [f64; FB] = std::array::from_fn(|u| ep[u] + (tp[u] - ep[u]) * n);
        r.path = Some(a);
        r.pts = th(&a);
        return r;
    }
    let o = e.pts.len().max(t.pts.len()).max(2);
    r.pts = (0..o).map(|i| v2_lerp(al(&e.pts, i, o), al(&t.pts, i, o), n)).collect();
    r
}
/// `ss` — a stadium/oval summary `{cx,cy,ang,halfLen,halfW}`.
fn ss(e: &Eye) -> (f64, f64, f64, f64, f64) {
    if e.is_oval() {
        let (o, s) = (e.oval[0], e.oval[1]);
        return (
            e.off[0],
            e.off[1],
            if o >= s { std::f64::consts::PI / 2.0 } else { 0.0 },
            o.max(s),
            o.min(s),
        );
    }
    let t = e.wid;
    if e.pts.len() < 2 {
        return (e.off[0], e.off[1], e.tilt, t, t);
    }
    let n = e.pts[0];
    let r = e.pts[e.pts.len() - 1];
    (
        e.off[0] + (n[0] + r[0]) / 2.0,
        e.off[1] + (n[1] + r[1]) / 2.0,
        (r[0] - n[0]).atan2(r[1] - n[1]) + e.tilt,
        hypot2(r[0] - n[0], r[1] - n[1]) / 2.0 + t,
        t,
    )
}
/// `nh` — the smallest roll between two tilts.
fn nh(e: f64, t: f64) -> f64 {
    let mut n = t - e;
    while n > std::f64::consts::PI / 2.0 {
        n -= std::f64::consts::PI;
    }
    while n < -std::f64::consts::PI / 2.0 {
        n += std::f64::consts::PI;
    }
    n
}
/// `Yb` — the stadium→oval ghost ring.
fn yb(e: f64, t: f64, n: f64) -> Vec<V2> {
    let o = 0.0f64.max(e - t);
    let s = n.clamp(0.0, 1.0);
    let mut i = Vec::new();
    for a in 0..48 {
        let c = a as f64 / 48.0 * std::f64::consts::TAU;
        let (u, l) = (c.cos(), c.sin());
        let (d, f) = (t * u, e * l);
        let h = t / u.abs().max(1e-6);
        let p = if h * l.abs() <= o + 1e-6 && h.is_finite() {
            h
        } else {
            let w = if l >= 0.0 { 1.0 } else { -1.0 } * o * l;
            let ss = 0.0f64.max(w * w - o * o + t * t);
            w + ss.sqrt()
        };
        let (g, m) = (p * u, p * l);
        i.push([g + (d - g) * s, m + (f - m) * s]);
    }
    i
}
/// `ua` — stadium of width e, height t at tilt n, offset r.
fn ua(e: f64, t: f64, n: f64, r: V2) -> Eye {
    sc(e.max(1e-4), t.max(e), 0.0, r, n)
}
/// `Zb` — stadium↔oval morph.
fn zb(e: &Eye, t: &Eye, n: f64) -> Eye {
    if n <= 1e-4 {
        return e.wt();
    }
    if n >= 0.995 {
        return t.wt();
    }
    let r = ss(e);
    let o = ss(t);
    let s = nh(r.2, o.2);
    let i = t.is_oval();
    let a = if i { 1.0 - (1.0 - n).powi(3) } else { n * n * n };
    let c = r.2 + s * n;
    let u = (r.3 + (o.3 - r.3) * n).max(1e-4);
    let l = (r.4 + (o.4 - r.4) * n).max(1e-4);
    let d = if e.is_oval() { 1.0 } else { 0.0 };
    let f = d + (if i { 1.0 } else { 0.0 } - d) * a;
    let mut h = ua(2.0 * l, 2.0 * u, c, [r.0 + (o.0 - r.0) * n, r.1 + (o.1 - r.1) * n]);
    let p = f > 1e-4;
    if p {
        h.oval = [l, u];
    }
    h.oval_mix = if p { f } else { 0.0 };
    h.fill_ring = Some(yb(u, l, f));
    h
}
/// `Qb` — stadium morph through a spine collapse.
fn qb_morph(e: &Eye, t: &Eye, n: f64) -> Eye {
    if n >= 1.0 - 1e-4 {
        return t.wt();
    }
    let r = ss(e);
    let o = ss(t);
    let s = r.2 + nh(r.2, o.2) * n;
    let i = (r.4 + (o.4 - r.4) * n).max(1e-4);
    let a = 0.0f64.max(r.3 - r.4) * (1.0 - n) + 0.0f64.max(o.3 - o.4) * n;
    let c = [r.0 + (o.0 - r.0) * n, r.1 + (o.1 - r.1) * n];
    sc(2.0 * i, 2.0 * (a + i), s * 180.0 / std::f64::consts::PI, c, 0.0)
}
/// `Jb` — the lid box of an eye.
fn jbx(e: &Eye) -> Option<(f64, f64, f64, f64, f64)> {
    let t = e.is_oval();
    if e.wid.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) && !t {
        return None;
    }
    let n = e.lid_y;
    let r = e.tilt * (180.0 / std::f64::consts::PI);
    if t {
        return Some((2.0 * e.oval[0], 2.0 * e.oval[1] * n, r, e.off[0], e.off[1]));
    }
    if e.pts.len() < 2 {
        return None;
    }
    let o = e.pts[0];
    let s = e.pts[e.pts.len() - 1];
    let (i, a) = (s[0] - o[0], s[1] - o[1]);
    let c = hypot2(i, a);
    let u = 2.0 * e.wid;
    Some((
        u,
        (c + u) * n,
        i.atan2(a) * (180.0 / std::f64::consts::PI) + r,
        e.off[0] + (o[0] + s[0]) / 2.0,
        e.off[1] + (o[1] + s[1]) / 2.0 * n,
    ))
}
/// `ex` — the lid box's unrotated extent.
fn ex(e: &Eye) -> (f64, f64, f64, f64) {
    let Some(t) = jbx(e) else {
        return (MR, 0.57, e.off[0], e.off[1]);
    };
    let n = t.2 * std::f64::consts::PI / 180.0;
    let (r, o) = (n.cos().abs(), n.sin().abs());
    (t.0 * r + t.1 * o, t.0 * o + t.1 * r, t.3, t.4)
}
const GB: f64 = 0.03;
const WB: f64 = 0.6;
const UB: f64 = 0.42;
const KB2: f64 = 20.0;

fn tx(e: f64, t: f64, n: f64, r: f64) -> Eye {
    if t >= e {
        ua(e, t, 0.0, [n, r])
    } else {
        ua(t, e, std::f64::consts::PI / 2.0, [n, r])
    }
}
/// `cl` — a lid closing on the eye (blink amount).
fn cl(e: &Eye, t: f64) -> Eye {
    if t <= 0.0 {
        return e.clone();
    }
    let n = t.min(1.0);
    let r = ex(e);
    let o = r.0.max(1e-4);
    let s = r.1.max(o);
    let i = o * UB;
    tx(o, s + (i - s) * n, r.2, r.3)
}
/// `nx` — the eye collapsed to its spine.
fn nx(e: &Eye) -> Eye {
    let mut n = e.clone();
    n.wid = 0.0;
    n.oval = [0.0, 0.0];
    n
}
/// `rx` — the eye's lid rest line.
fn rx(e: &Eye) -> f64 {
    let (t, n) = if e.is_oval() {
        (-e.oval[1], e.oval[1])
    } else {
        let (mut r, mut o) = (1e9f64, -1e9f64);
        for &s in &e.pts {
            if s[1] < r {
                r = s[1];
            }
            if s[1] > o {
                o = s[1];
            }
        }
        (r - e.wid, o + e.wid)
    };
    t + (1.0 - WB) * (n - t)
}
/// `ox` — normalize a mostly-vertical spine to horizontal.
fn ox_eye(e: &Eye) -> Eye {
    if e.pts.len() < 2 {
        return e.clone();
    }
    let t = e.pts[0];
    let n = e.pts[e.pts.len() - 1];
    let (r, o) = (n[0] - t[0], n[1] - t[1]);
    if o.abs() < r.abs() {
        return e.clone();
    }
    let s = r.atan2(o);
    if s.abs() < 1e-5 {
        return e.clone();
    }
    let (i, a) = (s.cos(), s.sin());
    let mut n = e.clone();
    n.tilt += s;
    n.pts = e.pts.iter().map(|c| [c[0] * i - c[1] * a, c[0] * a + c[1] * i]).collect();
    n
}
/// `ul` — a lid dropping onto the eye (`t` = how closed).
fn ul(e: &Eye, t: f64) -> Eye {
    if t == 0.0 {
        return e.clone();
    }
    let n = ox_eye(e);
    let r = GB.max(1.0 - t);
    let o = rx(&n) * (1.0 - r);
    let (s, i) = ((-n.tilt).cos(), (-n.tilt).sin());
    let mut out = n.clone();
    out.off = [n.off[0] - i * o, n.off[1] + s * o];
    out.lid_y = n.lid_y * r;
    out
}
/// `sx` — the inspect asymmetry (`o` bias −1..1 shrinks one eye).
fn sx(e: &[Eye; 2], t: f64, n: &Inspect, r: f64) -> [Eye; 2] {
    if t < 0.001 {
        return [e[0].clone(), e[1].clone()];
    }
    let o = if n.is_following_turn { (r / KB2).clamp(-1.0, 1.0) } else { n.bias };
    let [s, i] = [vn("inspecting")[0].clone(), vn("inspecting")[1].clone()];
    let (a, c) = (s.wid.max(1e-4), i.wid.max(1e-4));
    let u = (1.0 + o) / 2.0;
    let l = |d: &Eye, f: f64| -> Eye {
        let h = 1.0 + (f - 1.0) * t;
        let mut m = d.wt();
        m.wid *= h;
        m.pts = d.pts.iter().map(|p| [p[0] * h, p[1] * h]).collect();
        m
    };
    [l(&e[0], (c / a).powf(u)), l(&e[1], (a / c).powf(u))]
}

#[derive(Clone, Copy)]
pub(crate) struct Inspect {
    pub bias: f64,
    pub is_following_turn: bool,
}

// ---------------------------------------------------------------------------
// the expression machine — `ax` → Face
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum MorphPath {
    Oval,
    Spine,
    Stadium,
}

#[derive(Clone)]
enum FaceAnim {
    Morph {
        path: MorphPath,
        from_inspect: f64,
        to_name: &'static str,
        from: [Eye; 2],
        to: [Eye; 2],
        t: f64,
        dur: f64,
        held: bool,
    },
    LidMorph {
        from_inspect: f64,
        to_name: &'static str,
        from: [Eye; 2],
        to: [Eye; 2],
        t: f64,
        dur: f64,
        held: bool,
    },
}

impl FaceAnim {
    fn t(&self) -> f64 {
        match self {
            FaceAnim::Morph { t, .. } | FaceAnim::LidMorph { t, .. } => *t,
        }
    }
    fn dur(&self) -> f64 {
        match self {
            FaceAnim::Morph { dur, .. } | FaceAnim::LidMorph { dur, .. } => *dur,
        }
    }
    fn set_t(&mut self, v: f64) {
        match self {
            FaceAnim::Morph { t, .. } | FaceAnim::LidMorph { t, .. } => *t = v,
        }
    }
    fn held(&self) -> bool {
        match self {
            FaceAnim::Morph { held, .. } | FaceAnim::LidMorph { held, .. } => *held,
        }
    }
    fn set_held(&mut self, v: bool) {
        match self {
            FaceAnim::Morph { held, .. } | FaceAnim::LidMorph { held, .. } => *held = v,
        }
    }
    fn to_name(&self) -> &'static str {
        match self {
            FaceAnim::Morph { to_name, .. } | FaceAnim::LidMorph { to_name, .. } => to_name,
        }
    }
    fn inspect_from(&self) -> f64 {
        match self {
            FaceAnim::Morph { from_inspect, .. } | FaceAnim::LidMorph { from_inspect, .. } => *from_inspect,
        }
    }
}

/// `ax()` — the expression machine. `e` is the current face name, `t` the
/// settled eyes, `n` the settled inspect weight, `r` the running animation.
pub(crate) struct Face {
    e: &'static str,
    t: [Eye; 2],
    n: f64,
    r: Option<FaceAnim>,
}

fn dl(e: &FaceAnim) -> f64 {
    rc((e.t() / e.dur()).clamp(0.0, 1.0), BB)
}

impl Face {
    fn new() -> Face {
        Face { e: "neutral", t: vn("neutral"), n: 0.0, r: None }
    }
    /// `o()` — the eyes this frame.
    fn eyes(&self) -> [Eye; 2] {
        let Some(r) = &self.r else {
            return self.t.clone();
        };
        match r {
            FaceAnim::Morph { path, from, to, .. } => {
                let m = dl(r).clamp(0.0, 1.0);
                let go = |b: &Eye, v: &Eye| -> Eye {
                    match path {
                        MorphPath::Oval => zb(b, v, m),
                        MorphPath::Spine => xb(b, v, m),
                        MorphPath::Stadium => qb_morph(b, v, m),
                    }
                };
                [go(&from[0], &to[0]), go(&from[1], &to[1])]
            }
            FaceAnim::LidMorph { from, to, t, dur, .. } => {
                let f = (t / dur).min(1.0);
                let (h, p) = li(false, *dur);
                if f < h {
                    [ul(&from[0], ns(f / h)), ul(&from[1], ns(f / h))]
                } else if f <= p {
                    [nx(&to[0]), nx(&to[1])]
                } else {
                    let g = tb((f - p) / (1e-6f64).max(1.0 - p));
                    [ul(&to[0], 1.0 - g), ul(&to[1], 1.0 - g)]
                }
            }
        }
    }
    /// `s()` — the inspect weight this frame.
    fn inspect_weight(&self) -> f64 {
        let Some(r) = &self.r else {
            return self.n;
        };
        let f = r.inspect_from();
        let h = if r.to_name() == "inspecting" { 1.0 } else { 0.0 };
        if let FaceAnim::Morph { .. } = r {
            return f + (h - f) * dl(r);
        }
        let p = (r.t() / r.dur()).min(1.0);
        let (g, m) = li(false, r.dur());
        if p <= g {
            f
        } else if p < m {
            0.0
        } else {
            h
        }
    }
    fn is_morphing(&self) -> bool {
        self.r.as_ref().is_some_and(|r| !r.held())
    }
    fn name(&self) -> &'static str {
        self.e
    }
    /// `i` — a morph to expression `f`.
    fn morph(&mut self, f: &'static str) {
        let h = [self.eyes()[0].wt(), self.eyes()[1].wt()];
        let p = vn(f);
        let g = h.iter().any(Eye::is_bowed)
            || p.iter().any(Eye::is_bowed)
            || self.e == "sleepy"
            || f == "sleepy";
        // `ix` — oval only when nothing bows and a side is an oval.
        let path = if !h.iter().any(Eye::is_bowed)
            && !p.iter().any(Eye::is_bowed)
            && (h.iter().any(Eye::is_oval) || p.iter().any(Eye::is_oval))
        {
            MorphPath::Oval
        } else if g {
            MorphPath::Spine
        } else {
            MorphPath::Stadium
        };
        self.r = Some(FaceAnim::Morph {
            path,
            from_inspect: self.inspect_weight(),
            to_name: f,
            from: h,
            to: p,
            t: 0.0,
            dur: XB,
            held: false,
        });
        self.e = f;
    }
    /// `a` — a lid drop morph (through-blink expressions only).
    fn lid_morph(&mut self, f: &'static str) {
        self.r = Some(FaceAnim::LidMorph {
            from_inspect: self.inspect_weight(),
            to_name: f,
            from: [self.eyes()[0].wt(), self.eyes()[1].wt()],
            to: vn(f),
            t: 0.0,
            dur: IA,
            held: false,
        });
        self.e = f;
    }
    /// `u` — an expression track event.
    fn play(&mut self, name: &'static str, through_blink: bool) {
        if name == self.e && self.r.as_ref().is_none_or(|r| r.to_name() == name) {
            return;
        }
        if through_blink && !dr(name) && !dr(self.e) {
            self.lid_morph(name);
        } else {
            self.morph(name);
        }
    }
    /// `l` — settle animation `f` at local time `h`.
    fn settle(&mut self, f: &FaceAnim, h: f64) {
        if h >= f.dur() {
            self.t = match f {
                FaceAnim::Morph { to, .. } | FaceAnim::LidMorph { to, .. } => [to[0].wt(), to[1].wt()],
            };
            self.n = if f.to_name() == "inspecting" { 1.0 } else { 0.0 };
            self.r = None;
        } else {
            self.r = if h < 0.0 {
                None
            } else {
                let mut f = f.clone();
                f.set_t(h);
                Some(f)
            };
        }
    }
    /// `d` — hold flag.
    fn hold(&mut self, f: bool) {
        if let Some(r) = &mut self.r {
            r.set_held(f);
        }
    }
    /// `step(dt)` — advance the live animation.
    fn step(&mut self, dt: f64) {
        if self.r.as_ref().is_some_and(|r| !r.held()) {
            let r = self.r.clone().unwrap();
            let t = r.t() + dt;
            self.settle(&r, t);
        }
    }
    /// `release` — unhold.
    fn release(&mut self) {
        self.hold(false);
    }
    /// `seek(events, h)` — replay the expression script to time `h`, held.
    fn seek(&mut self, events: &[clips::ExprKey], h: f64) {
        self.e = "neutral";
        self.t = vn("neutral");
        self.n = 0.0;
        self.r = None;
        let mut p = 0.0;
        for m in events {
            if m.t > h + 1e-9 {
                break;
            }
            if let Some(r) = self.r.clone() {
                self.settle(&r, m.t - p);
            }
            self.play(m.name, m.through_blink);
            p = m.t;
        }
        if let Some(r) = self.r.clone() {
            self.settle(&r, h - p);
        }
        self.hold(true);
    }
}

// ---------------------------------------------------------------------------
// RNG — `ic` (mulberry-ish) and `rP` (FNV seed)
// ---------------------------------------------------------------------------

pub(crate) struct Rand(i32);
impl Rand {
    fn new(e: u32) -> Rand {
        Rand(e as i32)
    }
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(1831565813);
        let mut n = (self.0 ^ ((self.0 as u32) >> 15) as i32).wrapping_mul(self.0 | 1);
        n ^= n.wrapping_add((n ^ ((n as u32) >> 7) as i32).wrapping_mul(n | 61));
        ((n ^ ((n as u32) >> 14) as i32) as u32) as f64 / 4294967296.0
    }
}
/// `rP` — FNV-1a seed hash.
pub(crate) fn rp(e: &str) -> u32 {
    let mut t: u32 = 2166136261;
    for &b in e.as_bytes() {
        t = (t ^ b as u32).wrapping_mul(16777619);
    }
    t
}

// ---------------------------------------------------------------------------
// the effect engine — `Ix` → MorphFx
// ---------------------------------------------------------------------------

const SX: f64 = std::f64::consts::PI;
const MX: f64 = 0.1;
const AX: f64 = 1.5;

/// `vx` — the state-morph → fx-kind table.
pub(crate) fn vx(e: &str) -> Option<&'static str> {
    Some(match e {
        "thinking" => "dots",
        "orbit" => "orbit",
        "radar" => "radar",
        "progress" => "progress",
        "spawning" => "gather",
        "dictating" => "wave",
        "sending" => "send",
        "receiving" => "receive",
        "uploading" => "dock",
        "bouncing" => "ball",
        "writing" => "pencil",
        "alerting" => "bang",
        "powering-down" => "standby",
        _ => return None,
    })
}
/// `Ex` — morphs that loop with a rest between cycles.
fn ex_loop(e: &str) -> bool {
    matches!(e, "progress" | "spawning")
}
/// `ac` — a looping morph's cycle seconds.
fn ac(e: &str) -> f64 {
    match e {
        "progress" => 2.5,
        "spawning" => 2.0,
        _ => 2.5,
    }
}

/// `Ix.snapshot()` — what the frame sees of the effect engine.
#[derive(Clone)]
pub(crate) struct MorphSnap {
    pub amount: f64,
    pub spin: f64,
    pub kind: Option<&'static str>,
    pub outgoing: Option<&'static str>,
    pub swap: f64,
    pub clock: (f64, f64),
    pub outgoing_clock: (f64, f64),
    pub is_reduced_motion: bool,
    pub is_settled: bool,
}

struct MorphFx {
    reduced: bool,
    rng: Rand,
    amount: Spring,
    swap: Spring,
    spin: Spring,
    i: Option<&'static str>,
    a: Option<&'static str>,
    c: Option<&'static str>,
    u: Option<&'static str>,
    l: bool,
    d: f64,
    f: f64,
    h: f64,
    p: bool,
    g: f64,
    m: f64,
    y: f64,
    w: f64,
    s0: f64,
}

impl MorphFx {
    fn new(reduced: bool, seed: u32) -> MorphFx {
        MorphFx {
            reduced,
            rng: Rand::new(seed),
            amount: ao(0.0),
            swap: ao(1.0),
            spin: ao(0.0),
            i: None,
            a: None,
            c: None,
            u: None,
            l: false,
            d: 0.0,
            f: 0.0,
            h: 1.0,
            p: false,
            g: 0.0,
            m: 0.0,
            y: 0.0,
            w: 0.0,
            s0: 0.0,
        }
    }
    /// `b` — retarget bookkeeping, run before each advance/settle.
    fn b(&mut self) {
        let v = self.i.and_then(vx);
        let ee = self.y;
        if self.i != self.a {
            self.a = self.i;
            if self.i.is_some() {
                self.y = self.g;
                self.l = false;
            }
        }
        let mut want = v.is_some();
        if let Some(i) = self.i
            && ex_loop(i) {
                if !self.l && self.g - self.y > ac(i) {
                    self.l = true;
                    self.d = self.g;
                } else if self.l && self.g - self.d > AX {
                    self.l = false;
                    self.y = self.g;
                }
                want = !self.l;
        }
        self.amount.t = if want { 1.0 } else { 0.0 };
        if v.is_some() && v != self.c {
            if self.c.is_some() && self.amount.x > 0.02 {
                self.u = self.c;
                self.w = self.m;
                self.s0 = ee;
                self.swap.x = if self.reduced { 1.0 } else { 0.0 };
            } else {
                self.u = None;
                self.swap.x = 1.0;
            }
            self.swap.v = 0.0;
            self.swap.t = 1.0;
            self.c = v;
            self.m = self.g;
        }
        if v.is_none() && self.amount.x < 0.004 {
            self.c = None;
            self.u = None;
        }
        if self.swap.x > 0.996 {
            self.u = None;
        }
        if want != self.p {
            if want && !self.reduced {
                self.h = if self.rng.next() < 0.5 { 1.0 } else { -1.0 };
            }
            if !self.reduced {
                self.f += SX * self.h;
                self.spin.t = self.f;
            }
            self.p = want;
        }
    }
    fn set(&mut self, v: Option<&'static str>) {
        self.i = v;
    }
    fn advance(&mut self, v: f64) {
        let ee = if v > 0.0 { v.min(MX) } else { 0.0 };
        self.g += ee;
        self.b();
        let a = qf(ee);
        let f = ee / a as f64;
        for _ in 0..a {
            io(&mut self.amount, 14.0, 1.0, f);
            io(&mut self.swap, 11.0, 1.0, f);
            io(&mut self.spin, 14.0, 1.0, f);
        }
        if self.reduced {
            self.swap.x = 1.0;
            self.spin.x = self.spin.t;
            self.amount.x = self.amount.t;
        }
    }
    fn settle(&mut self) {
        self.b();
        for v in [&mut self.amount, &mut self.swap, &mut self.spin] {
            v.x = v.t;
            v.v = 0.0;
        }
        self.b();
    }
    fn snapshot(&self) -> MorphSnap {
        MorphSnap {
            amount: le(self.amount.x, 0.0, 1.0),
            spin: self.spin.x,
            kind: self.c,
            outgoing: self.u,
            swap: le(self.swap.x, 0.0, 1.0),
            clock: (self.g - self.m, self.g - self.y),
            outgoing_clock: (self.g - self.w, self.g - self.s0),
            is_reduced_motion: self.reduced,
            is_settled: (self.amount.x - self.amount.t).abs() < 0.001
                && (self.spin.t - self.spin.x).abs() < 0.01
                && self.swap.x > 0.996,
        }
    }
}

// ---------------------------------------------------------------------------
// pokes — `UN`/`rh`/`px`/`mo`
// ---------------------------------------------------------------------------

/// `UN` — the poke kinds.
pub(crate) const UN: &[&str] = &["spin", "bounce", "nod"];
const CX2: f64 = 0.9;
const UX: f64 = 0.4;
const LX: f64 = 0.62;
const DX: f64 = 0.3;
const FX: f64 = 0.5;
const HX: f64 = 16.0;

struct Poke {
    kind: &'static str,
    t: f64,
    turns: f64,
}
fn rh(p: &Poke) -> f64 {
    match p.kind {
        "spin" => CX2 + UX * (p.turns - 1.0),
        "bounce" => LX,
        _ => FX,
    }
}
fn mo(e: f64, t: f64, n: f64) -> f64 {
    if e <= t || e >= n {
        0.0
    } else {
        (std::f64::consts::PI * (e - t) / (n - t)).sin().powi(2)
    }
}
struct PokePose {
    pitch: f64,
    yaw: f64,
    lift: f64,
    squash: V2,
}
fn px(e: &[Poke]) -> PokePose {
    let (mut t, mut n, mut r, mut o, mut s) = (0.0, 0.0, 0.0, 1.0, 1.0);
    for i in e {
        let a = (i.t / rh(i)).min(1.0);
        match i.kind {
            "spin" => n += 180.0 * i.turns * (1.0 - (std::f64::consts::PI * a).cos()),
            "nod" => t += HX * mo(a, 0.0, 1.0),
            "bounce" => {
                let c = mo(a, 0.0, 0.18) + mo(a, 0.82, 1.0);
                let u = mo(a, 0.18, 0.82);
                r += DX * (std::f64::consts::PI * le((a - 0.14) / 0.72, 0.0, 1.0)).sin();
                o *= 1.0 + 0.08 * c - 0.04 * u;
                s *= 1.0 - 0.1 * c + 0.06 * u;
            }
            _ => {}
        }
    }
    PokePose { pitch: t, yaw: n, lift: r, squash: [o, s] }
}

// ---------------------------------------------------------------------------
// gaze — `bx`/`fl`/`mx`/`gx`/`yx`/`kx`
// ---------------------------------------------------------------------------

const MXG: f64 = 0.14;
const GXG: f64 = 1.0 / 60.0;
const YX: f64 = 0.55;
fn bx(e: f64) -> f64 {
    1.0 - (1.0 - MXG).powf(e / GXG)
}
struct GazePose {
    rx: f64,
    ry: f64,
    px: f64,
    py: f64,
}
fn fl(e: &str, t: f64, n: f64) -> GazePose {
    let r = e != "sphere";
    let o = match e {
        "heartChubby" => 0.6,
        "wedge" => 0.55,
        _ => 1.0,
    };
    GazePose {
        rx: -(if r { 19.0 } else { 14.0 }) * o * n,
        ry: (if r { 26.0 } else { 18.0 }) * o * t,
        px: (if r { 0.07 } else { 0.03 }) * t,
        py: (if r { 0.055 } else { 0.024 }) * n,
    }
}

// ---------------------------------------------------------------------------
// the clip player — `ih` → Mark
// ---------------------------------------------------------------------------

const ON: f64 = 0.6;
const RX: [f64; 4] = [0.4, 0.0, 0.2, 1.0];
const BX_LOOK: &[(&str, V3, V2)] = &[
    ("left", [0.0, -20.0, 0.0], [-0.1, 0.0]),
    ("right", [0.0, 20.0, 0.0], [0.1, 0.0]),
    ("up", [-20.0, 0.0, 0.0], [0.0, 0.1]),
    ("down", [20.0, 0.0, 0.0], [0.0, -0.1]),
    ("center", [0.0, 0.0, 0.0], [0.0, 0.0]),
];
const HX2: [f64; 4] = [0.42, 0.0, 0.58, 1.0];
const VX: f64 = 0.5;
const GX: f64 = 0.1;
const WX: f64 = 10.0;
const UX2: f64 = 1e-4;
const KX: f64 = 0.5;
const XX: f64 = 0.001;

/// A runtime key: `zx`'s `Nx` output (values owned so `qx` can generate them).
#[derive(Clone)]
struct RtKey {
    t: f64,
    v: Vec<f64>,
    ease: [f64; 4],
}
/// A runtime expression event: `zx`'s `{t,name,isThroughBlink}`.
#[derive(Clone)]
struct RExprKey {
    t: f64,
    name: &'static str,
    through_blink: bool,
}
/// A runtime clip — `zx` output (or `qx`'s bridge, or `hl`'s rest pose).
#[derive(Clone)]
struct RunClip {
    duration: f64,
    body: Option<&'static str>,
    squash_anchor: f64,
    rest_time: f64,
    inspect_bias: Option<f64>,
    inspect_follow: Option<bool>,
    position: Vec<RtKey>,
    rotation: Vec<RtKey>,
    squash: Vec<RtKey>,
    zoom: Vec<RtKey>,
    lids: Vec<RtKey>,
    expression: Vec<RExprKey>,
}
impl RunClip {
    fn from_static(c: &'static clips::Clip) -> RunClip {
        let keys = |ks: &'static [clips::TrackKey]| -> Vec<RtKey> {
            ks.iter().map(|k| RtKey { t: k.t, v: k.v.to_vec(), ease: k.ease }).collect()
        };
        RunClip {
            duration: c.duration,
            body: Some(c.body),
            squash_anchor: c.squash_anchor,
            rest_time: c.rest_time,
            inspect_bias: if c.inspect_bias.is_nan() { None } else { Some(c.inspect_bias) },
            inspect_follow: Some(c.inspect_follow),
            position: keys(c.position),
            rotation: keys(c.rotation),
            squash: keys(c.squash),
            zoom: keys(c.zoom),
            lids: keys(c.lids),
            expression: c
                .expression
                .iter()
                .map(|e| RExprKey { t: e.t, name: e.name, through_blink: e.through_blink })
                .collect(),
        }
    }
    /// `sh` — the empty 10s pose `qx` spreads onto.
    fn empty() -> RunClip {
        RunClip {
            duration: 10.0,
            body: None,
            squash_anchor: 0.0,
            rest_time: 0.0,
            inspect_bias: None,
            inspect_follow: None,
            position: vec![],
            rotation: vec![],
            squash: vec![],
            zoom: vec![],
            lids: vec![],
            expression: vec![],
        }
    }
    /// `hl` — the rest clip.
    fn rest(rest_rot: V3) -> RunClip {
        let mut c = RunClip::empty();
        c.rotation = vec![RtKey { t: 0.0, v: rest_rot.to_vec(), ease: RR }];
        c
    }
}

/// `vt` — sample a track at time `t`.
fn vt(e: &[RtKey], t: f64) -> Option<Vec<f64>> {
    if e.is_empty() {
        return None;
    }
    if t <= e[0].t {
        return Some(e[0].v.clone());
    }
    let n = &e[e.len() - 1];
    if t >= n.t {
        return Some(n.v.clone());
    }
    let mut r = 0;
    while r < e.len() - 2 && e[r + 1].t <= t {
        r += 1;
    }
    let o = &e[r];
    let s = &e[r + 1];
    let i = (t - o.t) / (s.t - o.t).max(1e-9);
    let a = rc(i, o.ease);
    Some(o.v.iter().zip(&s.v).map(|(c, u)| c + (u - c) * a).collect())
}

/// `Pose` — `pl`/`te` output.
#[derive(Clone)]
struct Pose {
    rot: V3,
    pos: V2,
    scl: V2,
    zoom: f64,
    lids: f64,
    expr: &'static str,
    squash_anchor: f64,
}
/// `pl` — a clip's rest pose (at its `restTime`).
fn pl(e: &RunClip) -> Pose {
    let t = e.rest_time;
    let n = vt(&e.rotation, t);
    let r = vt(&e.position, t);
    let o = vt(&e.squash, t);
    let s = vt(&e.zoom, t);
    let i = vt(&e.lids, t);
    let mut a = "neutral";
    let mut c = false;
    for u in &e.expression {
        if !c {
            a = u.name;
            c = true;
        }
        if u.t <= t + 1e-9 {
            a = u.name;
        }
    }
    Pose {
        rot: n.map(|v| [v[0], v[1], v[2]]).unwrap_or([0.0; 3]),
        pos: r.map(|v| [v[0], v[1]]).unwrap_or([0.0; 2]),
        scl: o.map(|v| [v[0], v[1]]).unwrap_or([1.0; 2]),
        zoom: s.map(|v| v[0]).unwrap_or(ON),
        lids: i.map(|v| v[0].clamp(0.0, 1.0)).unwrap_or(0.0),
        expr: a,
        squash_anchor: e.squash_anchor,
    }
}
/// `gl` — the expression in effect at time `t`.
fn gl(e: &[RExprKey], t: f64) -> &'static str {
    let mut n = "neutral";
    for r in e {
        if r.t > t + 1e-9 {
            break;
        }
        n = r.name;
    }
    n
}
/// `di` — `e` carried to the nearest winding of `t`.
fn di(e: f64, t: f64) -> f64 {
    e + 360.0 * ((t - e) / 360.0).round()
}
/// `fi` — circular angle difference.
fn fi(e: f64, t: f64) -> f64 {
    (((e - t) % 360.0 + 540.0) % 360.0 - 180.0).abs()
}
/// `jx` — pose ≈ pose.
fn jx(e: &Pose, t: &Pose) -> bool {
    fi(e.rot[0], t.rot[0]) < 4.0
        && fi(e.rot[1], t.rot[1]) < 4.0
        && fi(e.rot[2], t.rot[2]) < 4.0
        && (e.pos[0] - t.pos[0]).abs() < 0.015
        && (e.pos[1] - t.pos[1]).abs() < 0.015
        && (e.scl[0] - t.scl[0]).abs() < 0.02
        && (e.scl[1] - t.scl[1]).abs() < 0.02
        && (e.zoom - t.zoom).abs() < 0.02
        && (e.lids - t.lids).abs() < 0.05
}
/// `qx` — the `.35s` bridge clip from pose `e` to pose `t`.
fn qx(e: &Pose, t: &Pose) -> RunClip {
    let n = |o: Vec<f64>, s: Vec<f64>| {
        vec![
            RtKey { t: 0.0, v: o, ease: RX },
            RtKey { t: IA, v: s, ease: RR },
        ]
    };
    let r = [di(t.rot[0], e.rot[0]), di(t.rot[1], e.rot[1]), di(t.rot[2], e.rot[2])];
    let mut c = RunClip::empty();
    c.duration = IA;
    c.squash_anchor = e.squash_anchor;
    c.position = n(e.pos.to_vec(), t.pos.to_vec());
    c.rotation = n(e.rot.to_vec(), r.to_vec());
    c.squash = n(e.scl.to_vec(), t.scl.to_vec());
    c.zoom = n(vec![e.zoom], vec![t.zoom]);
    c.lids = if e.lids == 0.0 && t.lids == 0.0 {
        vec![]
    } else {
        n(vec![e.lids], vec![t.lids])
    };
    c
}
/// `hi` — nearest multiple of 360.
fn hi(e: f64) -> f64 {
    360.0 * (e / 360.0).round()
}

/// A named clip the director asked `load` for (the `E`/`A` slots).
#[derive(Clone)]
struct Loaded {
    clip: RunClip,
    name: &'static str,
    is_playing: bool,
    phase: f64,
    is_rest: bool,
}

/// A deferred face command (`P` in `ih`).
enum FaceCmd {
    Play(&'static str, bool),
    Morph(&'static str),
    MorphOrLid(&'static str, bool),
    HoldFace,
}

struct Look {
    t: f64,
    from_rot: V3,
    to_rot: V3,
    from_pos: V2,
    to_pos: V2,
}

struct Gaze {
    is_held: bool,
    is_always: bool,
    x: f64,
    y: f64,
    cx: f64,
    cy: f64,
    hold: f64,
}

struct BodyBlend {
    body: &'static str,
    weight: f64,
}

/// The mark frame — `frame()`'s output, `Uh`'s input.
pub(crate) struct Frame {
    pub quat: V4,
    pub zoom: f64,
    pub offset: V2,
    pub squash: V2,
    pub squash_anchor: f64,
    pub eyes: Option<[Eye; 2]>,
    pub bodies: Vec<BodyBlend2>,
    pub morph: Option<MorphSnap>,
    /// `On*t/2/x` resolved by the renderer — not part of the JS frame; kept
    /// out: `BE` computes its own from `displaySize`.
    pub(crate) _p: (),
}
/// `me()` output row.
#[derive(Clone)]
pub(crate) struct BodyBlend2 {
    pub body: &'static str,
    pub weight: f64,
}

/// `ih` — the mark engine.
pub(crate) struct Mark {
    tab: std::rc::Rc<Tables>,
    t: f64,
    n: Face,
    r: &'static str,
    o: bool,
    s: Vec<BodyBlend>,
    i: Spring,
    a: MorphFx,
    c: Option<V3>,
    u: V3,
    l: V2,
    d: V2,
    f: f64,
    h: f64,
    p: Inspect,
    g: Option<Look>,
    m: Gaze,
    y: RunClip,
    w: f64,
    playing: bool,
    b: bool,
    v: bool,
    e_load: Option<Loaded>,
    a_load: Option<Loaded>,
    big_n: f64,
    big_r: u64,
    big_b: Option<&'static str>,
    big_p: Option<FaceCmd>,
    big_i: bool,
    pokes: Vec<Poke>,
}

impl Mark {
    /// `ih({body, zoomMul, isGazeAlways, isReducedMotion, seed})`.
    pub(crate) fn new(tab: &std::rc::Rc<Tables>, body: Option<&'static str>, zoom_mul: f64, gaze_always: bool, reduced: bool, seed: u32) -> Mark {
        let t = zoom_mul.clamp(0.2, 3.0);
        let r = body.unwrap_or("sphere");
        let mut m = Mark {
            tab: tab.clone(),
            t,
            n: Face::new(),
            r,
            o: body.is_some(),
            s: vec![],
            i: ao(1.0),
            a: MorphFx::new(reduced, seed),
            c: None,
            u: tab.rest_rot,
            l: [0.0, 0.0],
            d: [1.0, 1.0],
            f: 0.0,
            h: ON * t,
            p: Inspect { bias: -1.0, is_following_turn: true },
            g: None,
            m: Gaze { is_held: false, is_always: gaze_always, x: 0.0, y: 0.0, cx: 0.0, cy: 0.0, hold: 0.0 },
            y: RunClip::rest(tab.rest_rot),
            w: 0.0,
            playing: false,
            b: true,
            v: false,
            e_load: None,
            a_load: None,
            big_n: 0.0,
            big_r: 0,
            big_b: None,
            big_p: None,
            big_i: false,
            pokes: vec![],
        };
        m.tab = tab.clone();
        m
    }

    /// `O` — apply the current clip's tracks at `w`.
    fn apply_pose(&mut self) {
        if let Some(v) = vt(&self.y.position, self.w) {
            self.l = [v[0], v[1]];
        } else {
            self.l = [0.0, 0.0];
        }
        self.u = match vt(&self.y.rotation, self.w) {
            Some(q) => [q[0], q[1], q[2]],
            None => [0.0, 0.0, 0.0],
        };
        self.d = match vt(&self.y.squash, self.w) {
            Some(v) => [v[0], v[1]],
            None => [1.0, 1.0],
        };
        if let Some(tv) = vt(&self.y.zoom, self.w) {
            self.h = tv[0].clamp(0.25, 2.5) * self.t;
        }
    }
    fn lids_track(&self) -> f64 {
        vt(&self.y.lids, self.w).map(|v| v[0].clamp(0.0, 1.0)).unwrap_or(0.0)
    }
    /// `fe` — lids (0 while a no-blink face forces them open).
    fn fe(&self) -> f64 {
        if self.big_i {
            0.0
        } else {
            self.lids_track()
        }
    }
    /// `K` — track the lids-forced flag.
    fn k_update(&mut self) {
        if !vb(self.n.name()) || self.n.is_morphing() {
            self.big_i = true;
        } else if self.big_i && self.lids_track() == 0.0 {
            self.big_i = false;
        }
    }
    /// `H` — defer a face command while a lid is down.
    fn defer(&mut self, cmd: FaceCmd) {
        if self.fe() > 0.0 {
            self.big_p = Some(cmd);
            return;
        }
        self.run_cmd(cmd);
        self.k_update();
    }
    fn run_cmd(&mut self, cmd: FaceCmd) {
        match cmd {
            FaceCmd::Play(name, tb) => self.n.play(name, tb),
            FaceCmd::Morph(name) => self.n.morph(name),
            FaceCmd::MorphOrLid(name, tb) => {
                if tb && !dr(name) && !dr(self.n.name()) {
                    self.n.lid_morph(name);
                } else {
                    self.n.morph(name);
                }
            }
            FaceCmd::HoldFace => {
                let q = self.big_b.unwrap_or_else(|| {
                    if let Some(load) = &self.a_load {
                        gl(&load.clip.expression, 0.0)
                    } else {
                        gl(&self.y.expression, self.w)
                    }
                });
                if q != self.n.name() {
                    self.n.morph(q);
                }
            }
        }
    }
    /// `L` — fire expression events in `($, q]`.
    fn expr_events(&mut self, lo: f64, hi: f64) {
        if self.big_b.is_some() {
            return;
        }
        // collect then run — the borrow on y's events outlives calls.
        let events: Vec<(f64, &'static str, bool)> = self
            .y
            .expression
            .iter()
            .filter(|g| g.t > lo && g.t <= hi)
            .map(|g| (g.t, g.name, g.through_blink))
            .collect();
        for (_, name, tb) in events {
            self.defer(FaceCmd::Play(name, tb));
        }
    }
    /// `j` — seek to `$`.
    fn seek(&mut self, t: f64) {
        self.w = t;
        self.g = None;
        self.big_p = None;
        self.apply_pose();
        self.n.seek(&self.expr_cloned(), t);
        if let Some(b) = self.big_b {
            self.n.seek(&[clips::ExprKey { t: 0.0, name: b, through_blink: false }], 0.0);
        }
    }
    fn expr_cloned(&self) -> Vec<clips::ExprKey> {
        self.y
            .expression
            .iter()
            .map(|e| clips::ExprKey { t: e.t, name: e.name, through_blink: e.through_blink })
            .collect()
    }
    /// `te` — the current pose.
    fn te(&self) -> Pose {
        Pose {
            rot: self.u,
            pos: self.l,
            scl: self.d,
            zoom: self.h / self.t,
            lids: self.fe(),
            expr: self.n.name(),
            squash_anchor: self.f,
        }
    }
    /// `Z` — adopt a clip's params (no bridge).
    fn adopt(&mut self, c: &RunClip) {
        self.playing = false;
        self.y = c.clone();
        self.w = 0.0;
        self.f = c.squash_anchor;
        if let Some(body) = c.body && !self.o {
            self.r = body;
        }
        self.p = Inspect {
            bias: c.inspect_bias.unwrap_or(self.p.bias),
            is_following_turn: c.inspect_follow.unwrap_or(self.p.is_following_turn),
        };
    }
    /// `z` — load now.
    fn load_now(&mut self, ld: Loaded) {
        self.a_load = None;
        self.adopt(&ld.clip.clone());
        self.e_load = Some(Loaded { clip: ld.clip.clone(), name: ld.name, is_playing: ld.is_playing, phase: ld.phase, is_rest: ld.is_rest });
        self.b = true;
        if !ld.is_playing {
            self.seek(0.0);
            if ld.is_rest {
                self.m.hold = 0.0;
            }
            return;
        }
        self.playing = true;
        if ld.phase > 0.0 {
            let dur = ld.clip.duration;
            self.seek(ld.phase * dur);
        }
        self.n.release();
        if ld.phase <= 0.0 {
            self.expr_events(-1e-9, 0.0);
        }
    }
    /// `W` — bridge into `ld`.
    fn bridge_to(&mut self, ld: Loaded) {
        let q = self.te();
        let g = pl(&ld.clip);
        self.a_load = Some(ld);
        self.adopt(&qx(&q, &g));
        self.b = false;
        self.playing = true;
        if self.big_b.is_none() && q.expr != g.expr {
            self.defer(FaceCmd::Morph(g.expr));
        }
    }
    /// `Y` — at a loop end: re-bridge the same clip if the pose drifted.
    fn maybe_rebridge(&mut self) -> bool {
        let should = self.a_load.is_some()
            || self.e_load.is_none()
            || jx(&self.te(), &pl(&self.e_load.as_ref().unwrap().clip));
        if should {
            return false;
        }
        let e = self.e_load.clone().unwrap();
        self.bridge_to(Loaded { clip: e.clip, name: e.name, is_playing: true, phase: 0.0, is_rest: false });
        true
    }
    /// `Q` — advance clip time by `dt`.
    fn tick(&mut self, dt: f64) {
        let q = self.w;
        let mut g = q + dt;
        if g >= self.y.duration {
            self.expr_events(q, self.y.duration);
            if self.b {
                self.big_r += 1;
                if self.maybe_rebridge() {
                    return;
                }
                g -= self.y.duration;
                if g >= self.y.duration {
                    g = 0.0;
                }
                self.expr_events(-1e-9, g);
                self.w = g;
            } else {
                self.w = self.y.duration;
                self.playing = false;
                if let Some(a) = self.a_load.clone() {
                    self.load_now(a);
                }
            }
        } else {
            self.expr_events(q, g);
            self.w = g;
        }
        self.g = None;
        self.apply_pose();
    }
    /// `se` — gaze per `dt`.
    fn gaze_step(&mut self, dt: f64) {
        if self.v || (!self.m.is_held && self.m.hold == 0.0) {
            return;
        }
        let q = bx(dt);
        let ne = |a: f64, b: f64, k: f64| {
            let t = a + (b - a) * k;
            if (b - t).abs() < XX {
                b
            } else {
                t
            }
        };
        self.m.cx = ne(self.m.cx, self.m.x, q);
        self.m.cy = ne(self.m.cy, self.m.y, q);
        self.m.hold = ne(self.m.hold, if self.m.is_held { 1.0 } else { 0.0 }, q);
        if self.playing {
            return;
        }
        self.apply_pose();
        let g = fl(self.r, self.m.cx, self.m.cy);
        let hold = self.m.hold;
        self.u = [
            self.u[0] + (g.rx - self.u[0]) * hold,
            self.u[1] + (g.ry - self.u[1]) * hold,
            self.u[2] * (1.0 - hold),
        ];
        self.l = [
            self.l[0] + (g.px - self.l[0]) * hold,
            self.l[1] + (g.py - self.l[1]) * hold,
        ];
    }
    /// `ee` — look animation per `dt`.
    fn look_step(&mut self, dt: f64) {
        let Some(g) = &mut self.g else { return };
        let q = g.t + dt;
        let gg = (q / VX).min(1.0);
        let t = rc(gg, HX2);
        self.u = [
            g.from_rot[0] + (g.to_rot[0] - g.from_rot[0]) * t,
            g.from_rot[1] + (g.to_rot[1] - g.from_rot[1]) * t,
            g.from_rot[2] + (g.to_rot[2] - g.from_rot[2]) * t,
        ];
        self.l = [
            g.from_pos[0] + (g.to_pos[0] - g.from_pos[0]) * t,
            g.from_pos[1] + (g.to_pos[1] - g.from_pos[1]) * t,
        ];
        if gg >= 1.0 {
            self.u = g.to_rot;
            self.l = g.to_pos;
            self.g = None;
        } else {
            self.g.as_mut().unwrap().t = q;
        }
    }
    /// `me` — blended body weights.
    fn bodies(&self) -> Vec<BodyBlend2> {
        if self.s.is_empty() {
            return vec![BodyBlend2 { body: self.r, weight: 1.0 }];
        }
        let k = ts(le(self.i.x, 0.0, 1.0));
        let mut q: Vec<(&'static str, f64)> = Vec::new();
        let mut add = |b: &'static str, w: f64| {
            if let Some(e) = q.iter_mut().find(|e| e.0 == b) {
                e.1 += w;
            } else {
                q.push((b, w));
            }
        };
        for c in &self.s {
            add(c.body, c.weight * (1.0 - k));
        }
        add(self.r, k);
        let g: Vec<(&'static str, f64)> = q.into_iter().filter(|e| e.1 > UX2).collect();
        let t: f64 = g.iter().map(|e| e.1).sum();
        g.into_iter().map(|(b, w)| BodyBlend2 { body: b, weight: w / t }).collect()
    }
    /// `de` — start a body transition.
    fn de(&mut self, b: &'static str) {
        if b == self.r {
            return;
        }
        self.s = self.bodies().into_iter().map(|b2| BodyBlend { body: b2.body, weight: b2.weight }).collect();
        self.r = b;
        self.i = Spring { x: 0.0, v: 0.0, t: 1.0 };
    }
    /// `ve` — advance the blend spring.
    fn blend_step(&mut self, dt: f64) {
        if self.s.is_empty() {
            return;
        }
        let q = qf(dt);
        for _ in 0..q {
            io(&mut self.i, WX, 1.0, dt / q as f64);
        }
        if (self.i.x - 1.0).abs() < 0.001 && self.i.v.abs() < 0.01 {
            self.i = Spring { x: 1.0, v: 0.0, t: 1.0 };
            self.s = vec![];
        }
    }
    fn pe(&self) -> f64 {
        self.fe()
    }
    /// `be` — the eyes of the frame (`ge` = yaw for `sx`).
    fn be(&self, ge: f64, inspect_w: f64) -> [Eye; 2] {
        let g = sx(&self.n.eyes(), inspect_w, &self.p, ge);
        let t = self.pe();
        if t <= 1e-4 {
            g
        } else {
            [cl(&g[0], t), cl(&g[1], t)]
        }
    }

    // -- public API (ih's returned object) ----------------------------------

    pub(crate) fn is_playing(&self) -> bool {
        self.playing
    }
    #[allow(dead_code)]
    pub(crate) fn time(&self) -> f64 {
        self.w
    }
    #[allow(dead_code)]
    pub(crate) fn duration(&self) -> f64 {
        self.y.duration
    }
    pub(crate) fn loop_count(&self) -> u64 {
        self.big_r
    }
    #[allow(dead_code)]
    pub(crate) fn face(&self) -> &'static str {
        self.n.name()
    }
    pub(crate) fn is_settled(&self) -> bool {
        let g = self.v
            || (self.m.hold == if self.m.is_held { 1.0 } else { 0.0 }
                && (!self.m.is_held || (self.m.cx == self.m.x && self.m.cy == self.m.y)));
        !self.playing
            && self.pokes.is_empty()
            && self.s.is_empty()
            && self.g.is_none()
            && self.big_p.is_none()
            && !self.n.is_morphing()
            && self.a.snapshot().is_settled
            && g
    }
    /// `load(clip, {isPlaying, transition, phase})` — `clip` is a `CLIPS` entry.
    pub(crate) fn load(&mut self, c: &'static clips::Clip, is_playing: bool, bridge: bool, phase: f64) {
        // `clip.name || prev?.name || "recipe"` — a nameless clip
        // inherits the loaded one's name, else the recipe default.
        let name = if !c.name.is_empty() {
            c.name
        } else {
            self.e_load.as_ref().map(|e| e.name).unwrap_or("recipe")
        };
        let d = Loaded {
            clip: RunClip::from_static(c),
            name,
            is_playing,
            phase: le(phase, 0.0, 1.0) % 1.0,
            is_rest: false,
        };
        if bridge
            && self.e_load.is_some()
            && (self.a_load.is_some() || self.e_load.as_ref().unwrap().name != name)
        {
            self.bridge_to(d);
        } else {
            self.load_now(d);
        }
    }
    #[allow(dead_code)]
    pub(crate) fn pause(&mut self) {
        self.v = true;
        self.playing = false;
    }
    #[allow(dead_code)]
    pub(crate) fn stop(&mut self) {
        self.playing = false;
        self.seek(0.0);
    }
    pub(crate) fn rest(&mut self) {
        self.v = false;
        let ld = Loaded { clip: RunClip::rest(self.tab.rest_rot), name: "rest", is_playing: false, phase: 0.0, is_rest: true };
        if self.a_load.is_none() && self.e_load.as_ref().is_none_or(|e| e.name == "rest") {
            if self.e_load.is_none() {
                self.load_now(ld);
            }
            return;
        }
        self.bridge_to(ld);
    }
    /// `setBody(body, {isInstant})`.
    pub(crate) fn set_body(&mut self, b: &'static str, instant: bool) {
        self.o = true;
        if !instant {
            self.de(b);
            return;
        }
        self.r = b;
        self.s = vec![];
        self.i = Spring { x: 1.0, v: 0.0, t: 1.0 };
    }
    /// `setMorph(morph, {isInstant})`.
    pub(crate) fn set_morph(&mut self, m: Option<&'static str>, instant: bool) {
        self.a.set(m);
        if instant {
            self.a.settle();
        }
    }
    /// `setExpression(name, {isThroughBlink})`.
    #[allow(dead_code)]
    pub(crate) fn set_expression(&mut self, e: &'static str, through_blink: bool) {
        let e = hb(e);
        self.defer(FaceCmd::MorphOrLid(e, through_blink));
    }
    /// `holdFace(name)`.
    pub(crate) fn hold_face(&mut self, e: Option<&'static str>) {
        if e == self.big_b {
            return;
        }
        self.big_b = e;
        self.defer(FaceCmd::HoldFace);
    }
    /// `poke(kind, {turns})`.
    pub(crate) fn poke(&mut self, kind: &'static str, turns: f64) {
        self.pokes.push(Poke { kind, t: 0.0, turns: turns.round().max(1.0) });
    }
    /// `setInspect({bias, isFollowingTurn})`.
    #[allow(dead_code)]
    pub(crate) fn set_inspect(&mut self, bias: Option<f64>, follow: Option<bool>) {
        self.p = Inspect {
            bias: bias.map_or(self.p.bias, |b| b.clamp(-1.0, 1.0)),
            is_following_turn: follow.unwrap_or(self.p.is_following_turn),
        };
    }
    /// `setGaze([x,y] | null)`.
    pub(crate) fn set_gaze(&mut self, g: Option<V2>) {
        match g {
            None => self.m.is_held = false,
            Some([x, y]) => {
                let (x, y) = (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0));
                if self.m.hold == 0.0 {
                    self.m.cx = x;
                    self.m.cy = y;
                }
                self.m.is_held = true;
                self.m.x = x;
                self.m.y = y;
            }
        }
    }
    #[allow(dead_code)]
    pub(crate) fn set_gaze_always(&mut self, v: bool) {
        self.m.is_always = v;
    }
    /// `look(dir)` — `Bx`.
    #[allow(dead_code)]
    pub(crate) fn look(&mut self, dir: &str) {
        for &(name, rot, pos) in BX_LOOK {
            if name == dir {
                self.g = Some(Look { t: 0.0, from_rot: self.u, to_rot: rot, from_pos: self.l, to_pos: pos });
                return;
            }
        }
    }
    /// `advance(dt)` — one clock tick.
    pub(crate) fn advance(&mut self, dt: f64) {
        let q = dt.min(GX);
        self.gaze_step(q);
        let g = 1.0 - self.a.snapshot().amount;
        if self.playing {
            self.tick(q * g);
        }
        self.look_step(q);
        self.n.step(q);
        if self.big_p.is_some() && self.fe() == 0.0 {
            let c = self.big_p.take().unwrap();
            self.run_cmd(c);
        }
        self.k_update();
        self.blend_step(q);
        for p in &mut self.pokes {
            p.t += q;
        }
        self.pokes.retain(|p| p.t < rh(p));
        self.a.advance(q);
        let t = self.a.snapshot();
        if t.amount > 0.0 || !t.is_settled {
            if self.c.is_none() {
                self.c = Some([hi(self.u[0]), hi(self.u[1]), hi(self.u[2])]);
            }
        } else {
            self.c = None;
        }
        self.big_n += dt;
    }
    /// `frame()` — the snapshot `Uh` renders.
    pub(crate) fn frame(&self) -> Frame {
        let gaze = if self.m.is_always && self.playing && !self.v {
            Some(fl(self.r, self.m.cx, self.m.cy))
        } else {
            None
        };
        let q = YX * self.m.hold;
        let g = px(&self.pokes);
        let t = [
            self.u[0] + gaze.as_ref().map_or(0.0, |g| g.rx * q) + g.pitch,
            self.u[1] + gaze.as_ref().map_or(0.0, |g| g.ry * q) + g.yaw,
            self.u[2],
        ];
        let c = [
            self.l[0] + gaze.as_ref().map_or(0.0, |g| g.px * q),
            self.l[1] + gaze.as_ref().map_or(0.0, |g| g.py * q) + g.lift,
        ];
        let d = [self.d[0] * g.squash[0], self.d[1] * g.squash[1]];
        let u = self.n.inspect_weight();
        let v = self.a.snapshot();
        let x = 1.0 - v.amount;
        let ie = (v.spin % (2.0 * std::f64::consts::PI)) / MN;
        let he = self.c.unwrap_or([0.0, 0.0, 0.0]);
        let pe = |ue: usize| he[ue] + (t[ue] - he[ue]) * x;
        let ge = pe(1);
        Frame {
            quat: ib(pe(0) * MN, (ge + ie) * MN, pe(2) * MN),
            zoom: self.h,
            offset: if x == 1.0 { c } else { [c[0] * x, c[1] * x] },
            squash: if x == 1.0 { d } else { [1.0 + (d[0] - 1.0) * x, 1.0 + (d[1] - 1.0) * x] },
            squash_anchor: self.f,
            eyes: if v.amount >= KX { None } else { Some(self.be(ge, u)) },
            bodies: self.bodies(),
            morph: if v.kind.is_none() && v.is_settled { None } else { Some(v) },
            _p: (),
        }
    }
}

// ---------------------------------------------------------------------------
// the director — `rv` → Director
// ---------------------------------------------------------------------------

fn ev(name: &str) -> &'static clips::Clip {
    clips::CLIPS
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("No motion recipe named {name}"))
}
fn yl(name: &str) -> &'static clips::StateDef {
    clips::STATES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, d)| d)
        .unwrap_or(&clips::STATES[0].1)
}
/// `nv` — shuffle a clip list, first entry differing from `p`.
fn nv(e: &[&'static str], r: &mut Rand, p: Option<&str>) -> Vec<&'static str> {
    let mut v = e.to_vec();
    for s in (1..v.len()).rev() {
        let n = (r.next() * (s + 1) as f64).floor() as usize;
        v.swap(s, n);
    }
    if let Some(o) = v.iter().position(|s| Some(*s) != p)
        && o > 0 {
            v.swap(0, o);
    }
    v
}

/// `rv` — the state director driving a `Mark`.
pub(crate) struct Director {
    rng: Rand,
    orders: HashMap<Vec<&'static str>, Vec<&'static str>>,
    s_compact: bool,
    i_resting: bool,
    a: bool,
    c: bool,
    u: &'static str,
    l: &'static clips::StateDef,
    d: f64,
    f: usize,
    h: u64,
    p: Option<&'static str>,
}

impl Director {
    /// `rv(mark, state, {seed, isCompact, isResting, firstPhase})`.
    pub(crate) fn new(
        mark: &mut Mark,
        state: &'static str,
        seed: u32,
        compact: bool,
        resting: bool,
        first_phase: Option<f64>,
    ) -> Director {
        let mut o = Director {
            rng: Rand::new(seed),
            orders: HashMap::new(),
            s_compact: compact,
            i_resting: resting,
            a: false,
            c: resting,
            u: state,
            l: yl(state),
            d: 0.0,
            f: 0,
            h: mark.loop_count(),
            p: None,
        };
        if resting {
            mark.rest();
        }
        o.set_state(mark, state, first_phase);
        o
    }
    fn clips_of(&self) -> &'static [&'static str] {
        if self.s_compact
            && let Some(c) = self.l.compact_clips {
                return c;
        }
        self.l.clips
    }
    /// `m` — a `next`-without-`holdSeconds` state plays its list in order.
    fn sequential(&self) -> bool {
        self.l.next.is_some() && self.l.hold_seconds == 0.0
    }
    /// `y` — next clip from the shuffled order.
    fn next_clip(&mut self) -> Option<&'static str> {
        let v = self.clips_of();
        let e = self.orders.entry(v.to_vec()).or_default();
        if e.is_empty() {
            e.extend(nv(v, &mut self.rng, self.p));
        }
        e.first().copied().inspect(|_| {
            e.remove(0);
        })
    }
    /// `w` — load the next clip.
    fn load_next(&mut self, mark: &mut Mark, first_phase: Option<f64>) {
        let v = if self.sequential() {
            self.clips_of().get(self.f).copied()
        } else {
            self.next_clip()
        };
        self.h = mark.loop_count();
        let Some(v) = v else { return };
        if Some(v) == self.p {
            return;
        }
        let unphased = self.c || self.sequential();
        mark.load(
            ev(v),
            true,
            true,
            if unphased { 0.0 } else { first_phase.unwrap_or_else(|| self.rng.next()) },
        );
        self.c = true;
        self.p = Some(v);
        self.h = mark.loop_count();
    }
    /// `S` — the state's held face.
    fn apply_face(&mut self, mark: &mut Mark) {
        let face = if let Some(f) = self.l.face {
            Some(f)
        } else if self.a {
            Some("surprised")
        } else {
            None
        };
        mark.hold_face(face);
    }
    /// `b` — switch state.
    fn set_state(&mut self, mark: &mut Mark, v: &'static str, first_phase: Option<f64>) {
        self.u = v;
        self.l = yl(v);
        self.d = 0.0;
        self.f = 0;
        mark.set_morph(self.l.morph, false);
        self.apply_face(mark);
        if !self.i_resting {
            self.load_next(mark, first_phase);
        }
    }
    #[allow(dead_code)]
    pub(crate) fn clip(&self) -> Option<&'static str> {
        self.p
    }
    pub(crate) fn set_state_pub(&mut self, mark: &mut Mark, v: &'static str) {
        if v != self.u {
            self.set_state(mark, v, None);
        }
    }
    pub(crate) fn set_compact(&mut self, v: bool) {
        self.s_compact = v;
    }
    pub(crate) fn set_emphasis(&mut self, mark: &mut Mark, v: bool) {
        if v == self.a {
            return;
        }
        self.a = v;
        self.apply_face(mark);
    }
    pub(crate) fn set_resting(&mut self, mark: &mut Mark, v: bool) {
        if v == self.i_resting {
            return;
        }
        self.i_resting = v;
        if !v {
            self.load_next(mark, None);
            return;
        }
        mark.rest();
        self.p = None;
    }
    /// `advance(dt)` — chain/hold bookkeeping.
    pub(crate) fn advance(&mut self, mark: &mut Mark, dt: f64) {
        if self.i_resting {
            return;
        }
        self.d += dt;
        if let Some(next) = self.l.next
            && self.l.hold_seconds > 0.0 && self.d >= self.l.hold_seconds {
                self.set_state(mark, next, None);
                return;
        }
        if mark.loop_count() != self.h {
            if let Some(next) = self.l.next
                && self.l.hold_seconds == 0.0 {
                    self.f += 1;
                    if self.f >= self.clips_of().len() {
                        self.set_state(mark, next, None);
                        return;
                    }
            }
            self.load_next(mark, None);
        }
    }
}

// ---------------------------------------------------------------------------
// public tables
// ---------------------------------------------------------------------------

/// `YN` — a user-facing shape name to the internal body.
pub(crate) fn yn(e: &str) -> &'static str {
    match e {
        "blob" => "sphere",
        "flower" => "starFlower",
        "heart" => "heartChubby",
        "star" => "star6",
        "sphere" => "sphere",
        "cloud" => "cloud",
        "square" => "square",
        "sparkle" => "sparkle",
        "clover" => "clover",
        "heartChubby" => "heartChubby",
        "starFlower" => "starFlower",
        "teardrop" => "teardrop",
        "tablet" => "tablet",
        "wedge" => "wedge",
        "house" => "house",
        "star6" => "star6",
        "hex" => "hex",
        _ => "sphere",
    }
}

/// A `STATES` name for `s` (`"idle"` when unknown) — lets
/// `Director::set_state` take run phases and preview names alike.
pub(crate) fn state_name(s: &str) -> &'static str {
    clips::STATES
        .iter()
        .find(|(n, _)| *n == s)
        .map(|(n, _)| *n)
        .unwrap_or("idle")
}

/// The picker shapes (`ol` in blob.tsx).
pub(crate) const BOT_SHAPES: &[&str] = &[
    "blob", "cloud", "square", "sparkle", "clover", "heart", "flower", "teardrop", "tablet",
    "wedge", "house", "star", "hex",
];
/// The picker colors (`Wf` minus black).
pub(crate) const BOT_COLORS: &[&str] = &[
    "#FFFFFF", "#A16B39", "#FC2C3E", "#F9670E", "#F69412", "#14C370", "#13B8A3", "#1884FC",
    "#905AFB", "#FB3499", "#777777",
];
/// `LEGACY_COLORS` — bundle values → rendered values.
pub(crate) fn norm_color(c: &str) -> String {
    let up = c.trim().to_uppercase();
    match up.as_str() {
        "#936439" => "#A16B39".to_string(),
        "#FF263C" => "#FC2C3E".to_string(),
        "#FF6700" => "#F9670E".to_string(),
        "#FF9800" => "#F69412".to_string(),
        "#00C972" => "#14C370".to_string(),
        "#00BCA6" => "#13B8A3".to_string(),
        "#1084FE" => "#1884FC".to_string(),
        "#9159FE" => "#905AFB".to_string(),
        "#FF309B" => "#FB3499".to_string(),
        _ => up,
    }
}
/// `inkOf` — the adaptive ink of a swatch (`appearance` decides white→black).
pub(crate) fn ink_of(c: &str, light: bool) -> String {
    let n = norm_color(c);
    if n == "#FFFFFF" && light {
        "#000000".to_string()
    } else {
        n
    }
}
/// `darken` — `rgb()` scaled by `1-k`.
pub(crate) fn darken(c: &str, k: f64) -> String {
    let n = norm_color(c);
    let hex = n.trim_start_matches('#');
    if hex.len() < 6 {
        return n;
    }
    let Ok(v) = u32::from_str_radix(&hex[..6], 16) else {
        return n;
    };
    let r = ((v >> 16) & 255) as f64 * (1.0 - k);
    let g = ((v >> 8) & 255) as f64 * (1.0 - k);
    let b = (v & 255) as f64 * (1.0 - k);
    format!("rgb({},{},{})", r.round() as i32, g.round() as i32, b.round() as i32)
}