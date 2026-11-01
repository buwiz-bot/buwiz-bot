//! The renderer — `Uh` and everything it calls, ported from the extracted
//! `mark/motion.mjs` section. Three branches: `BE`/`HE`/`VE`/`DE`/`KE`/`WE`
//! for the general/morph path, `HT`/`jT`/`qh`/`VT` for the sphere, `EM`/`AM`
//! for atlas bodies, plus the eye SDF rasterizer (`TT`/`ET`/`PT`/`AT`/`IT`/
//! `pT`/`mT`/`Dh`/`gT`/`ya`/`yT`/`zh`/`kT`/`pd`/`wT`/`_T`), the ellipse clip
//! machinery (`JM`/`ld`/`dd`/`hd`/`Br`/`fd`/`rT`/`oT`/`nT`/`eT`/`tT`/`qr`/
//! `Fh`/`sT`/`iT`/`aT`/`cT`/`BT`/`ka`/`wa`/`DM`), the body/eye polar blending
//! (`OM`/`LM`/`NM`/`UM`/`KM`/`fs`/`XM`/`YM`/`ZM`/`WM`/`FM`/`zM`/`jM`/`qM`/
//! `BM`/`HM`/`Lh`/`VM`/`GM`/`Nh`/`KE`/`XE`), the morph effect parts (`Ht`/`$s`/
//! `Vh`/`gE`/`TE`/`EE`/`PE`/`AE`/`IE`/`_E`/`CE`/`RE`/`$E`/`OE`/`LE`/`NE`/`FE`),
//! the morph driver (`DE`/`GE`/`WE`/`Ai`/`Gh`/`kE`/`wE`/`bE`/`xE`/`wc`/`Wh`/
//! `vE`/`SE`/`ME`), and `MarkTree`/`re`/`kc`/`un`/`ud`/`qt`/`Me`/`ga`.
//!
//! Skipped: the `isProof`/`glow` machinery (`hM`/`CM`/`WT`/`UT`/`Ei`/`oM`/
//! `rM`/`iM`/`yM`/`Ph`/`yc`/`kM`/`wM`/`bM`/`yd`/`KT`/`Ti`/`kd`/`XT`/`ba`) —
//! the frame never carries `glow` and callers never pass `isProof`, so the
//! branches are unreachable.

use std::cell::RefCell;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::rc::Rc;

use super::bodies::{self, Rot, X};
use super::{
    cr, et, jf, le, ts, wb, ye, Eye, EyeFrame, Frame, M3, MorphSnap, Tables, V2, V3,
};

const TAU: f64 = PI * 2.0;
const UT: f64 = TAU;
const CD: f64 = 1e-9;
const RM: f64 = 1e-7;
const M_DOLLAR: f64 = 1e-7;
const TN: usize = 720;
const H_UND: f64 = 184.0 / 256.0; // `_h`
const IH: f64 = 144.0;
const ON: f64 = 0.6;
const DOLLAR_O: f64 = 180.0 / PI; // `$o`
const TM: f64 = 40.0;
const ZT2: f64 = PI / 48.0; // `zT`
const DT: usize = 192;
const JH: f64 = PI * 0.999;
const UT2: usize = 60;
const LT: usize = 256;
const DT2: usize = 24;
const FT2: usize = 2e4 as usize;
const HT2: f64 = 0.2;
const BT2: f64 = 0.02;
const XT2: i32 = 16;
const VT2: f64 = 0.015;
const ST2: f64 = 400.0;
const MT2: usize = 512;
const HS: usize = 5;
const YT2: f64 = -15.0 + 259.0 / 2.0;
const ZT_M: f64 = 0.62;
const JT3: f64 = 22.0;
const VD: f64 = 62.0;
const EE2: f64 = 0.84;
const TE2: f64 = 0.22;
const NE2: f64 = 1.02;
const RE2: f64 = 9.0;
const OE2: f64 = 1400.0;
const BH: f64 = 1500.0;
const XA: f64 = 1700.0;
pub(crate) const PS: f64 = 2500.0;
pub(crate) const PR: f64 = 0.68;
const OO: f64 = PS * PR;
pub(crate) const SE: f64 = 2.4;
const IE2: f64 = 120.0;
const QE: f64 = 0.6;
const UE_ARR: [f64; 4] = [-2.0, -1.0, 1.0, 2.0];

// the morph factor/zoom tables (`Pi`/`xd`)
fn pi(e: &str) -> f64 {
    match e {
        "dots" => 22.0,
        "orbit" | "radar" | "progress" | "gather" => 19.0,
        "wave" => 16.0,
        "send" | "receive" | "dock" => 20.0,
        "ball" => 18.0,
        "pencil" => 17.0,
        "bang" | "standby" => 13.0,
        _ => 19.0,
    }
}
fn xd(e: &str) -> f64 {
    match e {
        "dots" => 1.5,
        "orbit" | "radar" => 1.14,
        "progress" => 1.32,
        "gather" => 1.15,
        "wave" => 1.42,
        "send" | "receive" => 1.12,
        "dock" => 1.3,
        "ball" => 1.22,
        "pencil" => 1.18,
        "bang" => 1.28,
        "standby" => 1.75,
        _ => 1.0,
    }
}

/// `ac` — a looping morph's cycle seconds.
fn ac(e: &str) -> f64 {
    match e {
        "progress" => 2.5,
        "spawning" => 2.0,
        _ => 2.5,
    }
}
fn ae2() -> f64 {
    ac("progress") * 1e3
}
fn ce2() -> f64 {
    ac("spawning") * 1e3
}

/// `ic` — mulberry32 factory.
fn ic(e: f64) -> impl FnMut() -> f64 {
    let mut t = e as i32;
    move || {
        t = t.wrapping_add(1831565813);
        let mut n = (t ^ ((t as u32) >> 15) as i32).wrapping_mul(t | 1);
        n ^= n.wrapping_add((n ^ ((n as u32) >> 7) as i32).wrapping_mul(n | 61));
        (((n ^ ((n as u32) >> 14) as i32) as u32) as f64) / 4294967296.0
    }
}

/// `Bt` — unit wrap.
fn bt(e: f64) -> f64 {
    (e % 1.0 + 1.0) % 1.0
}

// ---------------------------------------------------------------------------
// formatters + MarkTree
// ---------------------------------------------------------------------------

fn un(e: f64) -> String {
    format!("{}", (e * 100.0).round() / 100.0)
}
fn ud(e: f64) -> String {
    un(e)
}
fn qt(e: f64) -> String {
    un(e)
}
fn me(e: f64, t: u32) -> String {
    let n = 10f64.powi(t as i32);
    format!("{}", (e * n).round() / n)
}
fn xo(e: f64) -> f64 {
    (e * 1e6).round() / 1e6
}
/// `fE` — the pencil's capsule path (30×88, centered on `X`).
pub(crate) fn f_e(e: f64, t: f64) -> String {
    let n = e / 2.0;
    let r = X - t / 2.0 + n;
    let o = X + t / 2.0 - n;
    format!(
        "M{} {}A{} {} 0 0 1 {} {}L{} {}A{} {} 0 0 1 {} {}Z",
        X - n, r, n, n, X + n, r, X + n, o, n, n, X - n, o
    )
}

/// `hE` — the bang stem's tapered capsule path.
pub(crate) fn h_e(e: f64, t: f64, n: f64) -> String {
    let r = e / 2.0;
    let o = t / 2.0;
    let s = X - n / 2.0;
    let i = X + n / 2.0;
    format!(
        "M{} {}A{} {} 0 0 1 {} {}L{} {}A{} {} 0 0 1 {} {}Z",
        X - r, s + r, r, r, X + r, s + r, X + o, i - o, o, o, X - o, i - o
    )
}

fn ga(e: &[V2]) -> String {
    if e.is_empty() {
        return String::new();
    }
    let mut d = format!("M{} {}", ud(e[0][0]), ud(e[0][1]));
    for p in &e[1..] {
        d += &format!("L{} {}", ud(p[0]), ud(p[1]));
    }
    d + "Z"
}

/// A virtual-SVG node (`paintTree` walks `tag`/`attrs`/`children`).
pub(crate) struct MarkTree {
    pub tag: &'static str,
    pub attrs: Vec<(&'static str, String)>,
    pub children: Vec<MarkTree>,
}

impl MarkTree {
    pub(crate) fn attr(&self, k: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.as_str())
    }
}

fn re(tag: &'static str, attrs: &[(&'static str, String)], children: Vec<MarkTree>) -> MarkTree {
    MarkTree {
        tag,
        attrs: attrs.to_vec(),
        children,
    }
}
fn r2(tag: &'static str, attrs: &[(&'static str, String)]) -> MarkTree {
    re(tag, attrs, vec![])
}
fn sv(e: &str) -> String {
    e.to_owned()
}

/// `kc` — the root svg node (`size` viewBox, `r` layout extent px).
fn kc(size: f64, id: &str, extent: f64, children: Vec<MarkTree>) -> MarkTree {
    re(
        "svg",
        &[
            ("xmlns", sv("http://www.w3.org/2000/svg")),
            ("id", sv(id)),
            ("width", format!("{}", (extent * 100.0).round() / 100.0)),
            ("height", format!("{}", (extent * 100.0).round() / 100.0)),
            ("viewBox", format!("0 0 {} {}", un(size), un(size))),
        ],
        children,
    )
}

// ---------------------------------------------------------------------------
// `PaintOpts`/`cc`/`uc`/`zr`/`UhSize` (`o` arg of `Uh`)
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
pub(crate) struct PaintOpts {
    pub ink: V3,
    pub spot: V3,
    pub cutout: bool,
    pub ink_paint: Option<String>,
    pub spot_paint: Option<String>,
}

fn zr(e: V3) -> String {
    format!(
        "#{:02X}{:02X}{:02X}",
        (e[0] * 255.0).round().clamp(0.0, 255.0) as u32,
        (e[1] * 255.0).round().clamp(0.0, 255.0) as u32,
        (e[2] * 255.0).round().clamp(0.0, 255.0) as u32
    )
}
fn cc(t: &PaintOpts) -> String {
    t.ink_paint.clone().unwrap_or_else(|| zr(t.ink))
}
fn uc(t: &PaintOpts) -> String {
    t.spot_paint.clone().unwrap_or_else(|| zr(t.spot))
}

/// `o` — `Uh`'s `{displaySize, extent}` record.
pub(crate) struct UhSize {
    pub display_size: Option<f64>,
    pub extent: f64,
}

// ---------------------------------------------------------------------------
// `Ch`/`Rh`/`sd`/`Oh`/`ad`/`EM`/`AM` — the atlas branch
// ---------------------------------------------------------------------------

/// `Rh` — the angle-quantum table for a display size (snaps the rot so the
/// sprite cache stays dense).
fn rh(e: f64, t: f64) -> f64 {
    let n = TM / (e * t / 2.0).max(1.0) * DOLLAR_O;
    2f64.powf(n.log2().floor())
}

/// `sd` — wrap to [0,180), nudging off the ±90 gimbal edge.
fn sd(e: f64) -> f64 {
    let t = (e % 180.0 + 180.0) % 180.0;
    if (t - 90.0).abs() >= 0.5 {
        e
    } else {
        e + if t <= 90.0 { 89.5 - t } else { 90.5 - t }
    }
}

/// `Ch` — quaternion → quantized `{yaw,pitch,roll}` degrees.
fn ch(e: &super::V4, t: f64) -> Rot {
    let n = cr(*e);
    let m = if n[8] >= 0.0 {
        n
    } else {
        [-n[0], n[1], -n[2], -n[3], n[4], -n[5], -n[6], n[7], -n[8]]
    };
    let [r, o, s] = jf(m);
    let i = |a: f64| if t > 0.0 { (a / t).round() * t } else { a };
    Rot {
        yaw: sd(i(o * DOLLAR_O)),
        pitch: sd(i(r * DOLLAR_O)),
        roll: i(s * DOLLAR_O),
    }
}

/// `Oh` — the sprite→screen squash/offset matrix `[a,b,c,d,e,f]` as
/// `[scaleX, scaleY, tx, ty]`.
fn oh(e: &Frame, t: f64, n: f64, r: f64) -> [f64; 4] {
    let o = t / 2.0;
    let s = o + e.offset[0] * e.zoom * o;
    let i = o - e.offset[1] * e.zoom * o;
    let a = i - e.squash_anchor * e.zoom * o;
    let [c, u] = e.squash;
    let l = n / r;
    [c * l, u * l, s - c * n / 2.0, a + u * (i - n / 2.0 - a)]
}

/// `ad` — path data → point loops (the `M..Z` loops `gc`/`ga` emit).
fn ad(e: &str) -> Vec<Vec<V2>> {
    let mut t = Vec::new();
    for n in e.split('Z') {
        let r: Vec<f64> = n
            .replace('M', " ")
            .split_whitespace()
            .filter_map(|s| s.parse::<f64>().ok())
            .collect();
        if r.len() < 6 {
            continue;
        }
        let mut o = Vec::new();
        let mut s = 0;
        while s + 1 < r.len() {
            o.push([r[s], r[s + 1]]);
            s += 2;
        }
        t.push(o);
    }
    t
}

/// `EM`'s record.
struct EmRec {
    size: f64,
    sprite: f64,
    transform: String,
    body: Vec<String>,
    body_fill: Option<String>,
    eyes: Vec<String>,
    eye_fill: Option<String>,
}

/// `EM` — the atlas render record (`body` is `s.body`, like `{...e,body}`).
fn em(e: &Frame, body: &'static str, t: &PaintOpts, n: f64) -> EmRec {
    let o = ((n * e.zoom / H_UND).round() as i64).max(8) as f64;
    let s = o.max(IH);
    let i = ch(&e.quat, rh(n, e.zoom));
    let sh = bodies::dollar_h(body, &i, s, e.eyes.as_ref());
    let [u, l, d, f] = oh(e, n, o, s);
    let h = format!("matrix({} 0 0 {} {} {})", xo(u), xo(l), xo(d), xo(f));
    let g = if t.cutout { None } else { Some(uc(t)) };
    let m = if e.eyes.is_none() {
        vec![]
    } else {
        sh.eyes
    };
    EmRec {
        size: n,
        sprite: s,
        transform: h,
        body: sh.body,
        body_fill: Some(cc(t)),
        eyes: m,
        eye_fill: g,
    }
}

/// `AM` — the atlas branch tree.
fn am(e: &EmRec, t: &str, n: f64) -> MarkTree {
    let r = e.sprite;
    let o = format!("{}-body", t);
    let i = |f: &str| -> Vec<MarkTree> {
        e.eyes
            .iter()
            .map(|h| r2("path", &[("d", h.clone()), ("fill", sv(f))]))
            .collect()
    };
    let a: Vec<MarkTree> = e
        .body
        .iter()
        .map(|f| r2("path", &[("d", f.clone())]))
        .collect();
    let mut c = vec![re(
        "g",
        &[("id", o.clone())],
        a,
    )];
    let mut u_mask: Option<String> = None;
    let mut l: Vec<MarkTree> = Vec::new();
    if !e.eyes.is_empty() && e.eye_fill.is_none() {
        let holes = format!("{}-holes", t);
        let mut kids = vec![r2(
            "rect",
            &[
                ("width", un(r)),
                ("height", un(r)),
                ("fill", sv("#fff")),
            ],
        )];
        kids.extend(i("#000"));
        c.push(re(
            "mask",
            &[
                ("id", holes.clone()),
                ("maskUnits", sv("userSpaceOnUse")),
                ("x", sv("0")),
                ("y", sv("0")),
                ("width", un(r)),
                ("height", un(r)),
            ],
            kids,
        ));
        u_mask = Some(format!("url(#{})", holes));
    } else if !e.eyes.is_empty() {
        let inside = format!("{}-inside", t);
        c.push(re(
            "mask",
            &[
                ("id", inside.clone()),
                ("maskUnits", sv("userSpaceOnUse")),
                ("x", sv("0")),
                ("y", sv("0")),
                ("width", un(r)),
                ("height", un(r)),
            ],
            vec![r2(
                "use",
                &[("href", format!("#{}", o)), ("fill", sv("#fff"))],
            )],
        ));
        l.push(re(
            "g",
            &[("mask", format!("url(#{})", inside))],
            i(&e.eye_fill.clone().unwrap_or_default()),
        ));
    }
    let mut d_attrs: Vec<(&'static str, String)> = vec![
        ("href", format!("#{}", o)),
        ("fill", e.body_fill.clone().unwrap_or_default()),
    ];
    if let Some(m) = u_mask {
        d_attrs.push(("mask", m));
    }
    let d = r2("use", &d_attrs);
    kc(
        e.size,
        t,
        n,
        vec![
            re("defs", &[], c),
            re("g", &[("transform", e.transform.clone())], {
                let mut v = vec![d];
                v.extend(l);
                v
            }),
        ],
    )
}

// ---------------------------------------------------------------------------
// the eye SDF rasterizer (`ET`/`_T`/`AT`/`PT`/`IT`/`pT`/`mT`/`Dh`/`gT`/`ya`/
// `yT`/`zh`/`kT`/`pd`/`wT`/`TT`)
// ---------------------------------------------------------------------------

/// `ET` — the eye's raster record.
struct EtRec {
    segments: Vec<f64>, // 5 per segment: x y dx dy invLenSq
    half_width: f64,
    rx: f64,
    ry: f64,
    ellipse_mix: f64,
}

/// `_T` — smoothstep in [e,t].
fn _t(e: f64, t: f64, n: f64) -> f64 {
    let r = ((n - e) / (t - e)).clamp(0.0, 1.0);
    r * r * (3.0 - 2.0 * r)
}

fn et_rec(e: &Eye) -> EtRec {
    let [t, n] = e.oval;
    let o = if t > 1e-5 && n > 1e-5 {
        if e.oval_mix > 1e-4 {
            e.oval_mix.min(1.0)
        } else {
            _t(0.0, 0.08, t.min(n))
        }
    } else {
        0.0
    };
    let s = e.pts.len().saturating_sub(1).max(0);
    let mut i = Vec::with_capacity(s * HS);
    for a in 0..s {
        let [c, u] = e.pts[a];
        let l = e.pts[a + 1][0] - c;
        let d = e.pts[a + 1][1] - u;
        let f = l * l + d * d;
        i.extend([c, u, l, d, if f > 0.0 { 1.0 / f } else { 0.0 }]);
    }
    EtRec {
        segments: i,
        half_width: if e.wid > 0.0 { e.wid } else { -1.0 },
        rx: t,
        ry: n,
        ellipse_mix: o,
    }
}

/// `AT` — distance to the polyline segments.
fn at(e: &[f64], t: f64, n: f64) -> f64 {
    let mut r = 1e6f64;
    let mut o = 0usize;
    while o + 4 < e.len() + 1 && o + HS <= e.len() {
        let s = t - e[o];
        let i = n - e[o + 1];
        let a = e[o + 2];
        let c = e[o + 3];
        let u = (s * a + i * c) * e[o + 4];
        let l = u.clamp(0.0, 1.0);
        let d = s - a * l;
        let f = i - c * l;
        let h = d * d + f * f;
        if h < r {
            r = h;
        }
        o += HS;
    }
    r.sqrt()
}

/// `PT` — the eye field.
fn pt_field(e: &EtRec) -> impl Fn(f64, f64) -> f64 + '_ {
    move |t: f64, n: f64| -> f64 {
        let r = at(&e.segments, t, n) - e.half_width;
        if e.ellipse_mix == 0.0 {
            return r;
        }
        let o = t / e.rx;
        let s = n / e.ry;
        let i = ((o * o + s * s).sqrt() - 1.0) * e.rx.min(e.ry);
        r * (1.0 - e.ellipse_mix) + i * e.ellipse_mix
    }
}

struct PtConf<'a> {
    field: &'a dyn Fn(f64, f64) -> f64,
    tolerance_dev: f64,
    tolerance_edge: f64,
    root: f64,
    reach: f64,
}

struct Grid {
    x0: f64,
    y0: f64,
    step: f64,
    nx: usize,
    ny: usize,
}

/// `Dh` — the regula-falsi root between (t,n) and (o,s).
fn dh(e: &PtConf, t: f64, n: f64, r: f64, o: f64, s: f64, i: f64) -> f64 {
    let mut a = 0.0f64;
    let mut c = r;
    let mut u = 1.0f64;
    let mut l = i;
    let mut d = 0i32;
    let mut f = 0.5f64;
    for _ in 0..UT2 {
        f = (a * l - u * c) / (l - c);
        let p = (e.field)(t + (o - t) * f, n + (s - n) * f);
        if p.abs() <= e.root || u - a <= 1e-15 {
            break;
        }
        if (p < 0.0) == (l < 0.0) {
            u = f;
            l = p;
            if d == -1 {
                c /= 2.0;
            }
            d = -1;
        } else {
            a = f;
            c = p;
            if d == 1 {
                l /= 2.0;
            }
            d = 1;
        }
    }
    f
}

/// `zh` — raycast from (t,n) along (o,s) toward the `r`-signed surface.
fn zh(e: &PtConf, t: f64, n: f64, r: f64, o: f64, s: f64) -> Option<f64> {
    if r == 0.0 {
        return Some(0.0);
    }
    let mut i = 0.0f64;
    let mut a = r;
    for _ in 0..LT {
        let u = i + a.abs().max(e.root);
        if u > e.reach {
            return None;
        }
        let l = t + o * u;
        let d = n + s * u;
        let f = (e.field)(l, d);
        if (f < 0.0) != (r < 0.0) {
            return Some(i + (u - i) * dh(e, t + o * i, n + s * i, a, l, d, f));
        }
        i = u;
        a = f;
    }
    None
}

/// `yT` — a midpoint redirected along the gradient, or its normal fallback.
fn yt(e: &PtConf, t: f64, n: f64, r: f64, o: f64, s: f64, i: f64) -> V2 {
    let a = [t + o * i, n + s * i];
    let c = i * 0.001;
    let u = (e.field)(t + c, n) - (e.field)(t - c, n);
    let l = (e.field)(t, n + c) - (e.field)(t, n - c);
    let d = (u * u + l * l).sqrt();
    if d == 0.0 {
        return a;
    }
    let f = if u * s - l * o < 0.0 { -1.0 } else { 1.0 };
    let h = -l / d * f;
    let p = u / d * f;
    if h * o + p * s < HT2 {
        return a;
    }
    match zh(e, t, n, r, h, p) {
        Some(g) if g * (h * o + p * s) > i => [t + h * g, n + p * g],
        _ => a,
    }
}

/// `ya` — recursive midpoint subdivision of segment t→n into `o`.
fn ya(e: &PtConf, t: V2, n: V2, r: usize, o: &mut Vec<V2>) {
    let s = n[0] - t[0];
    let i = n[1] - t[1];
    let a = (s * s + i * i).sqrt();
    if a == 0.0 || r >= DT2 || o.len() >= FT2 {
        return;
    }
    let c = (t[0] + n[0]) / 2.0;
    let u = (t[1] + n[1]) / 2.0;
    let l = (e.field)(c, u);
    if !l.is_finite() {
        return;
    }
    let d = if l > 0.0 { 1.0 } else { -1.0 };
    let f = -i / a * d;
    let h = s / a * d;
    let p = zh(e, c, u, l, f, h);
    match p {
        None => {}
        Some(p) if p <= e.tolerance_dev / 2.0 && a <= e.tolerance_edge => {}
        Some(p) => {
            let g = if l > 0.0 {
                yt(e, c, u, l, f, h, p)
            } else {
                [c + f * p, u + h * p]
            };
            ya(e, t, g, r + 1, o);
            o.push(g);
            ya(e, g, n, r + 1, o);
        }
    }
}

/// `gT` — the subdivided polyline for one cell loop.
fn gtt(e: &PtConf, t: &[V2]) -> Vec<V2> {
    let mut n = Vec::new();
    for r in 0..t.len() {
        let o = t[r];
        let s = t[(r + 1) % t.len()];
        if o[0] == s[0] && o[1] == s[1] {
            continue;
        }
        n.push(o);
        ya(e, o, s, 0, &mut n);
    }
    n
}

/// `pd` — squared distance.
fn pd(e: V2, t: V2) -> f64 {
    let n = t[0] - e[0];
    let r = t[1] - e[1];
    n * n + r * r
}

/// `wT` — perpendicular distance of `e` to segment t→n.
fn wt(e: V2, t: V2, n: V2) -> f64 {
    let r = n[0] - t[0];
    let o = n[1] - t[1];
    let s = e[0] - t[0];
    let i = e[1] - t[1];
    let a = r * r + o * o;
    let c = if a > 0.0 {
        ((s * r + i * o) / a).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let u = s - r * c;
    let l = i - o * c;
    (u * u + l * l).sqrt()
}

/// `kT` — the RDP-ish simplifier against (`deviation`,`edge`).
fn kt(e: &[V2], dev: f64, edge: f64) -> Vec<V2> {
    let n = e.len();
    if n < 4 {
        return e.to_vec();
    }
    let mut r = vec![0u8; n];
    let mut o = 0usize;
    let mut s = -1.0f64;
    for a in 1..n {
        let c = pd(e[0], e[a]);
        if c > s {
            s = c;
            o = a;
        }
    }
    r[0] = 1;
    r[o] = 1;
    fn split(e: &[V2], r: &mut [u8], a: usize, c: usize, dev: f64, edge: f64) {
        if c - a < 2 {
            return;
        }
        let u = e[a];
        let l = e[c % e.len()];
        let mut d = -1.0f64;
        let mut f = a;
        for (p, &point) in e.iter().enumerate().take(c).skip(a + 1) {
            let g = wt(point, u, l);
            if g > d {
                d = g;
                f = p;
            }
        }
        let h = d > dev / 2.0;
        if !h && pd(u, l).sqrt() <= edge {
            return;
        }
        if !h {
            f = (a + c) >> 1;
        }
        r[f] = 1;
        split(e, r, a, f, dev, edge);
        split(e, r, f, c, dev, edge);
    }
    split(e, &mut r, 0, o, dev, edge);
    split(e, &mut r, o, n, dev, edge);
    e.iter()
        .enumerate()
        .filter_map(|(i, &p)| if r[i] == 1 { Some(p) } else { None })
        .collect()
}

/// `mT` — marching squares with edge-continuation links.
fn mt(e: &PtConf, t: &Grid) -> Vec<Vec<V2>> {
    let Grid { x0: r, y0: o, step: s, nx: i, ny: a } = *t;
    let c = i + 1;
    let mut u = vec![0.0f64; c * (a + 1)];
    for b in 0..=a {
        for v in 0..i {
            u[b * c + v] = (e.field)(r + v as f64 * s, o + b as f64 * s);
        }
    }
    let l = 2 * c * (a + 1);
    let mut d = vec![-1i64; l];
    let mut f: HashMap<usize, V2> = HashMap::new();
    let h = |b: usize, f: &mut HashMap<usize, V2>| -> V2 {
        if let Some(&v) = f.get(&b) {
            return v;
        }
        let ee = b >> 1;
        let aa = if b & 1 == 1 { ee + c } else { ee + 1 };
        let fx = r + (ee % c) as f64 * s;
        let fy = o + (ee / c) as f64 * s;
        let rx = r + (aa % c) as f64 * s;
        let ry = o + (aa / c) as f64 * s;
        let p = dh(e, fx, fy, u[ee], rx, ry, u[aa]);
        let ii = [fx + (rx - fx) * p, fy + (ry - fy) * p];
        f.insert(b, ii);
        ii
    };
    let mut m: Vec<usize> = Vec::with_capacity(4);
    let mut y: Vec<bool> = Vec::with_capacity(4);
    for b in 0..a {
        for v in 0..i {
            let ee = b * c + v;
            let p = [ee, ee + 1, ee + 1 + c, ee + c];
            let g = [2 * ee, 2 * (ee + 1) + 1, 2 * (ee + c), 2 * ee + 1];
            m.clear();
            y.clear();
            for aa in 0..4 {
                let f1 = u[p[aa]] < 0.0;
                let n1 = u[p[(aa + 1) & 3]] < 0.0;
                if f1 != n1 {
                    m.push(g[aa]);
                    y.push(f1);
                }
            }
            if m.len() == 2 {
                let aa = if y[0] { 0 } else { 1 };
                d[m[aa]] = m[1 - aa] as i64;
            } else if m.len() == 4 {
                let aa = (e.field)(r + (v as f64 + 0.5) * s, o + (b as f64 + 0.5) * s) < 0.0;
                for ff in 0..4 {
                    if y[ff] {
                        d[m[ff]] = m[(ff + if aa { 1 } else { 3 }) & 3] as i64;
                    }
                }
            }
        }
    }
    let mut w: Vec<Vec<V2>> = Vec::new();
    let mut seen = vec![0u8; l];
    for b in 0..l {
        if d[b] < 0 || seen[b] != 0 {
            continue;
        }
        let mut v: Vec<V2> = Vec::new();
        let mut ee = b as i64;
        while ee >= 0 && seen[ee as usize] == 0 {
            seen[ee as usize] = 1;
            v.push(h(ee as usize, &mut f));
            ee = d[ee as usize];
        }
        if ee == b as i64 {
            w.push(v);
        }
    }
    w
}

/// `pT` — the rasterizer driver.
fn pt(conf: &PtConf, t: &Grid, dev: f64, edge: f64) -> Vec<Vec<V2>> {
    let mut o = Vec::new();
    for s in mt(conf, t) {
        let i = kt(&gtt(conf, &s), dev, edge);
        if i.len() >= 3 {
            o.push(i);
        }
    }
    o
}

/// `IT` — the eye rasterizer: bound the segments, grid, `pT`.
fn it(e: &EtRec, t: f64) -> Vec<Vec<V2>> {
    let mut n = f64::INFINITY;
    let mut r = f64::INFINITY;
    let mut o = f64::NEG_INFINITY;
    let mut s = f64::NEG_INFINITY;
    let mut i = f64::INFINITY;
    let c = e.half_width;
    if c > 0.0 && e.ellipse_mix < 1.0 && !e.segments.is_empty() {
        let mut d = 0usize;
        while d + HS <= e.segments.len() {
            for k in 0..2usize {
                let f = e.segments[d] + e.segments[d + 2] * k as f64;
                let h = e.segments[d + 1] + e.segments[d + 3] * k as f64;
                n = n.min(f - c);
                r = r.min(h - c);
                o = o.max(f + c);
                s = s.max(h + c);
            }
            d += HS;
        }
        i = c;
    }
    if e.ellipse_mix > 0.0 {
        n = n.min(-e.rx);
        r = r.min(-e.ry);
        o = o.max(e.rx);
        s = s.max(e.ry);
        i = i.min(e.rx).min(e.ry);
    }
    if !(o > n && s > r) {
        return vec![];
    }
    let u = (VT2.min(i / 3.0)).max((o - n).max(s - r) / ST2);
    let l = Grid {
        x0: n - 2.0 * u,
        y0: r - 2.0 * u,
        step: u,
        nx: ((o - n) / u).ceil() as usize + 4,
        ny: ((s - r) / u).ceil() as usize + 4,
    };
    let field = pt_field(e);
    let conf = PtConf {
        field: &field,
        tolerance_dev: t,
        tolerance_edge: (4.0 * t).sqrt(),
        root: t * 0.001,
        reach: l.step * ((l.nx as f64).hypot(l.ny as f64)),
    };
    pt(&conf, &l, t, (4.0 * t).sqrt())
}

thread_local! {
    static WN: RefCell<HashMap<String, Rc<Vec<Vec<V2>>>>> = RefCell::new(HashMap::new());
}

/// `TT` — cached eye loops at pixel size `t`.
fn tt(e: &Eye, t: f64) -> Rc<Vec<Vec<V2>>> {
    let n = XT2.min(0i32.max((t.max(1.0)).log2().ceil() as i32));
    let key = format!(
        "{}|{}|{}|{}|{}|{}",
        n,
        e.wid,
        e.oval[0],
        e.oval[1],
        e.oval_mix,
        e.pts
            .iter()
            .map(|p| format!("{},{}", p[0], p[1]))
            .collect::<Vec<_>>()
            .join(";")
    );
    WN.with(|w| {
        let mut w = w.borrow_mut();
        if let Some(o) = w.get(&key) {
            return o.clone();
        }
        let s = Rc::new(it(&et_rec(e), BT2 / 2f64.powi(n)));
        w.insert(key.clone(), s.clone());
        if w.len() > MT2 {
            let k = w.keys().next().cloned();
            if let Some(k) = k {
                w.remove(&k);
            }
        }
        s
    })
}

// ---------------------------------------------------------------------------
// the ellipse/hemisphere clip machinery (`Seg`, `JM`, `ld`, `dd`, `hd`,
// `Br`, `fd`, `rT`, `oT`, `nT`, `eT`, `tT`, `qr`, `Fh`, `sT`, `iT`, `aT`,
// `cT`, `BT`, `ka`, `wa`, `DM`)
// ---------------------------------------------------------------------------

/// A path segment (`{kind:"line"|"arc", to, radii?, sweep?}`).
#[derive(Clone)]
pub(crate) enum Seg {
    Line { to: V2 },
    Arc { to: V2, radii: V2, sweep: f64 },
}

impl Seg {
    fn to(&self) -> V2 {
        match self {
            Seg::Line { to } => *to,
            Seg::Arc { to, .. } => *to,
        }
    }
}

/// `hd` — wrap angle to [-π,π].
fn hd(e: f64) -> f64 {
    e - TAU * (e / TAU).round()
}

/// `Br` — wrap angle to [0,2π).
fn br(e: f64) -> f64 {
    let t = e % TAU;
    if t < 0.0 {
        t + TAU
    } else {
        t
    }
}

/// `fd` — lerp point.
fn fd(e: V2, t: V2, n: f64) -> V2 {
    [e[0] + (t[0] - e[0]) * n, e[1] + (t[1] - e[1]) * n]
}

/// `ld` — the unit point where segment e→t crosses z=0.
fn ld(e: V3, t: V3) -> V2 {
    let n = e[2] / (e[2] - t[2]);
    let r = e[0] + (t[0] - e[0]) * n;
    let o = e[1] + (t[1] - e[1]) * n;
    let s = (r * r + o * o).sqrt();
    [r / s, o / s]
}

/// `dd` — the unit-circle arc between boundary points `e`→`t` at offset `n`.
fn dd(e: V2, t: V2, n: f64) -> Seg {
    let r = hd(t[1].atan2(t[0]) - n) - hd(e[1].atan2(e[0]) - n);
    Seg::Arc {
        to: t,
        radii: [1.0, 1.0],
        sweep: r,
    }
}

/// `JM` — clip a projected loop (`[x,y,z]` pts) to the front hemisphere.
fn jm(e: &[V3], t: f64) -> Option<Vec<Seg>> {
    let n = e.len();
    let mut r = -1i64;
    let mut a = 0usize;
    while a < n && r < 0 {
        if e[a][2] <= 0.0 && e[(a + 1) % n][2] > 0.0 {
            r = a as i64;
        }
        a += 1;
    }
    if r < 0 {
        return if n == 0 || e[0][2] <= 0.0 {
            None
        } else {
            Some(
                e.iter()
                    .map(|p| Seg::Line { to: [p[0], p[1]] })
                    .collect(),
            )
        };
    }
    let r = r as usize;
    let mut o: Vec<Seg> = Vec::new();
    let mut s = [1.0, 0.0];
    let mut i: Option<V2> = None;
    for a in 0..n {
        let c = e[(r + a) % n];
        let u = e[(r + a + 1) % n];
        let l = c[2] > 0.0;
        let d = u[2] > 0.0;
        if l && d {
            o.push(Seg::Line { to: [u[0], u[1]] });
        } else if l {
            let p = ld(c, u);
            i = Some(p);
            o.push(Seg::Line { to: p });
        } else if d {
            let f = ld(c, u);
            match i {
                None => s = f,
                Some(i) => o.push(dd(i, f, t)),
            }
            o.push(Seg::Line { to: [u[0], u[1]] });
        }
    }
    if let Some(i) = i {
        o.push(dd(i, s, t));
    }
    Some(o)
}

/// `rT` — line-segment ∩ ellipse params.
fn rt(e: V2, t: V2, n: f64, r: f64) -> Vec<f64> {
    let o = t[0] - e[0];
    let s = t[1] - e[1];
    let i = n * o * o + r * s * s;
    let a = n * e[0] * o + r * e[1] * s;
    let c = n * e[0] * e[0] + r * e[1] * e[1] - 1.0;
    let u = a * a - i * c;
    if i == 0.0 || u <= 0.0 {
        return vec![];
    }
    let l = u.sqrt();
    let d = -(a + if a < 0.0 { -l } else { l });
    let mut v: Vec<f64> = [d / i, c / d]
        .into_iter()
        .filter(|&f| f > 0.0 && f < 1.0)
        .collect();
    v.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// `oT` — arc-segment ∩ ellipse params.
fn ot(e: f64, t: f64, n: f64, r: f64, o: f64) -> Vec<f64> {
    if r == o || n == 0.0 {
        return vec![];
    }
    let s = (1.0 / (e * e) - o) / (r - o);
    if !(s > 0.0 && s < 1.0) {
        return vec![];
    }
    let i = s.sqrt().acos();
    let mut a: Vec<f64> = Vec::new();
    for c in [i, -i, PI - i, PI + i] {
        let l = if n > 0.0 { br(c - t) } else { br(t - c) } / n.abs();
        if l > 0.0 && l < 1.0 {
            a.push(l);
        }
    }
    a.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    a
}

/// `nT`'s record.
struct NTItem {
    from: V2,
    segment: Seg,
    sweep: f64,
    is_inside: bool,
}

/// `nT` — split the loop `e` at ellipse `t` crossings.
fn nt(e: &[Seg], t: V2) -> Vec<NTItem> {
    let n = 1.0 / (t[0] * t[0]);
    let r = 1.0 / (t[1] * t[1]);
    let o = |a: V2| n * a[0] * a[0] + r * a[1] * a[1] - 1.0;
    let mut s: Vec<NTItem> = Vec::new();
    let mut i = e[e.len() - 1].to();
    for a in e {
        match a {
            Seg::Line { to } => {
                let c = rt(i, *to, n, r);
                let mut u = i;
                for l in 0..=c.len() {
                    let d = if l == 0 { 0.0 } else { c[l - 1] };
                    let f = if l == c.len() { 1.0 } else { c[l] };
                    let h = if l == c.len() { *to } else { fd(i, *to, f) };
                    s.push(NTItem {
                        from: u,
                        segment: Seg::Line { to: h },
                        sweep: (u[0] * h[1] - u[1] * h[0])
                            .atan2(u[0] * h[0] + u[1] * h[1]),
                        is_inside: o(fd(i, *to, (d + f) / 2.0)) < 0.0,
                    });
                    u = h;
                }
            }
            Seg::Arc { to, radii, sweep } => {
                let c = radii[0];
                let u = i[1].atan2(i[0]);
                let l = ot(c, u, *sweep, n, r);
                let mut d = i;
                for f in 0..=l.len() {
                    let h = if f == 0 { 0.0 } else { l[f - 1] };
                    let p = if f == l.len() { 1.0 } else { l[f] };
                    let g = u + sweep * p;
                    let m = if f == l.len() {
                        *to
                    } else {
                        [c * g.cos(), c * g.sin()]
                    };
                    let y = u + sweep * (h + p) / 2.0;
                    let w = sweep * (p - h);
                    s.push(NTItem {
                        from: d,
                        segment: Seg::Arc {
                            to: m,
                            radii: *radii,
                            sweep: w,
                        },
                        sweep: w,
                        is_inside: o([c * y.cos(), c * y.sin()]) < 0.0,
                    });
                    d = m;
                }
            }
        }
        i = a.to();
    }
    s
}

/// `eT` — clip loop `e` to ellipse `t`.
fn et_clip(e: &[Seg], t: V2) -> Option<Vec<Seg>> {
    let n = nt(e, t);
    let r = n.len();
    if n.iter().all(|c| c.is_inside) {
        return Some(e.to_vec());
    }
    let mut o = -1i64;
    let mut c = 0usize;
    while c < r && o < 0 {
        if n[c].is_inside && !n[(c + r - 1) % r].is_inside {
            o = c as i64;
        }
        c += 1;
    }
    if o < 0 {
        let c: f64 = n.iter().map(|u| u.sweep).sum();
        return if c.abs() > PI {
            Some(qr(t, c.signum()))
        } else {
            None
        };
    }
    let o = o as usize;
    let mut s: Vec<Seg> = Vec::new();
    let mut i: Option<V2> = None;
    let mut a = 0.0f64;
    for c in 0..r {
        let u = &n[(o + c) % r];
        if u.is_inside {
            if i.is_some() {
                s.push(Seg::Arc {
                    to: u.from,
                    radii: t,
                    sweep: a,
                });
            }
            i = None;
            s.push(u.segment.clone());
        } else {
            if i.is_none() {
                i = Some(u.from);
                a = 0.0;
            }
            a += u.sweep;
        }
    }
    if let Some(i) = i {
        let _ = i;
        s.push(Seg::Arc {
            to: n[o].from,
            radii: t,
            sweep: a,
        });
    }
    Some(s)
}

/// `qr` — the full ellipse as two half arcs.
fn qr(e: V2, t: f64) -> Vec<Seg> {
    vec![
        Seg::Arc {
            to: [-e[0], 0.0],
            radii: e,
            sweep: t * PI,
        },
        Seg::Arc {
            to: [e[0], 0.0],
            radii: e,
            sweep: t * PI,
        },
    ]
}

/// `Fh` — the ellipse point at angle `t`.
fn fh2(e: V2, t: f64) -> V2 {
    let n = t.cos();
    let r = t.sin();
    let o = 1.0 / (n * n / (e[0] * e[0]) + r * r / (e[1] * e[1])).sqrt();
    [o * n, o * r]
}

/// `sT`'s belt record and `iT`'s.
struct Belt {
    from: V2,
    to: V2,
    segments: Vec<Seg>,
}

/// `sT` — reverse a run into a belt.
fn st(e: V2, t: &[Seg]) -> Belt {
    let mut n: Vec<V2> = vec![e];
    n.extend(t.iter().map(|o| o.to()));
    let mut r: Vec<Seg> = t
        .iter()
        .enumerate()
        .map(|(o, s)| match s {
            Seg::Line { .. } => Seg::Line { to: n[o] },
            Seg::Arc { radii, sweep, .. } => Seg::Arc {
                to: n[o],
                radii: *radii,
                sweep: -*sweep,
            },
        })
        .collect();
    r.reverse();
    Belt {
        from: n[n.len() - 1],
        to: e,
        segments: r,
    }
}

/// `aT` — does arc `e` cover angle `t`?
fn at2(e: &Belt, t: f64) -> f64 {
    // `e.sweep` — for `iT`'s synthetic records the belt stores a single arc.
    let Seg::Arc { sweep, .. } = e.segments[0] else {
        return 0.0;
    };
    let n = e.from[1].atan2(e.from[0]);
    let v = if sweep > 0.0 { br(t - n) } else { br(n - t) };
    if v < sweep.abs() {
        sweep.signum()
    } else {
        0.0
    }
}

/// `iT` — the missing boundary arcs between belt endpoints `n` on ellipse `e`.
fn it2(e: V2, t: &[Belt], n: &[V2]) -> Vec<Belt> {
    let mut r: Vec<(V2, f64)> = n
        .iter()
        .map(|s| (*s, br(s[1].atan2(s[0]))))
        .collect();
    r.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut o = Vec::new();
    for s in 0..r.len() {
        let i = r[s];
        let a = r[(s + 1) % r.len()];
        let c = if s + 1 < r.len() {
            a.1 - i.1
        } else {
            a.1 + TAU - i.1
        };
        if c <= 0.0 {
            continue;
        }
        let u = i.1 + c / 2.0;
        let covered: f64 = t.iter().map(|d| at2(d, u)).sum();
        if covered == 0.0 {
            o.push(Belt {
                from: i.0,
                to: a.0,
                segments: vec![Seg::Arc {
                    to: a.0,
                    radii: e,
                    sweep: c,
                }],
            });
        }
    }
    o
}

/// `cT` — connect belts into closed seg loops.
fn ct(e: Vec<Belt>) -> Vec<Vec<Seg>> {
    let t = |s: V2| format!("{} {}", s[0], s[1]);
    let mut n: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, s) in e.iter().enumerate() {
        n.entry(t(s.from)).or_default().push(idx);
    }
    let mut r: Vec<bool> = vec![false; e.len()];
    let mut o: Vec<Vec<Seg>> = Vec::new();
    for s in 0..e.len() {
        if r[s] {
            continue;
        }
        let mut i: Vec<Seg> = Vec::new();
        let mut a: Option<usize> = Some(s);
        while let Some(ai) = a {
            if r[ai] {
                break;
            }
            r[ai] = true;
            i.extend(e[ai].segments.iter().cloned());
            a = n.get(&t(e[ai].to)).and_then(|c| c.iter().copied().find(|c| !r[*c]));
        }
        if !i.is_empty() {
            o.push(i);
        }
    }
    o
}

/// `tT` — merge the ellipse boundary `e` into the loops `t` (for `cutout`).
fn tt2(e: V2, t: Vec<Vec<Seg>>) -> Vec<Vec<Seg>> {
    let n = |a: &Seg| matches!(a, Seg::Arc { radii, .. } if radii[0] == e[0] && radii[1] == e[1]);
    let mut r: Vec<Vec<Seg>> = Vec::new();
    let mut o: Vec<Belt> = Vec::new();
    let mut s: Vec<Belt> = Vec::new();
    let mut i: Vec<V2> = Vec::new();
    for a in t {
        let c = a.iter().position(&n);
        let Some(c) = c else {
            r.push(a);
            continue;
        };
        let mut u = a[c].to();
        let mut l: Vec<Seg> = Vec::new();
        let mut d = u;
        for f in 1..=a.len() {
            let h = &a[(c + f) % a.len()];
            if n(h) {
                let Seg::Arc { sweep, .. } = h else { unreachable!() };
                s.push(Belt {
                    from: d,
                    to: [0.0, 0.0],
                    segments: vec![Seg::Arc {
                        to: h.to(),
                        radii: e,
                        sweep: *sweep,
                    }],
                });
                i.push(d);
                i.push(h.to());
                if !l.is_empty() {
                    o.push(st(u, &l));
                    l = Vec::new();
                }
                u = h.to();
            } else {
                l.push(h.clone());
            }
            d = h.to();
        }
    }
    if s.is_empty() {
        let mut out = vec![qr(e, 1.0)];
        out.extend(r);
        out
    } else {
        let mut belts = o;
        belts.extend(it2(e, &s, &i));
        let mut out = ct(belts);
        out.extend(r);
        out
    }
}

/// `BT` — a seg loop → points (arcs sampled at `zT`).
fn bt_pts(e: &[Seg]) -> Vec<V2> {
    let mut t = Vec::new();
    let mut n = e.last().map(|s| s.to()).unwrap_or([0.0, 0.0]);
    for r in e {
        if let Seg::Arc { radii, sweep, .. } = r {
            let o = n[1].atan2(n[0]);
            let s = (sweep.abs() / ZT2).ceil().max(1.0) as usize;
            for i in 1..s {
                t.push(fh2(*radii, o + sweep * i as f64 / s as f64));
            }
        }
        t.push(r.to());
        n = r.to();
    }
    t
}

/// `wa` — an arc `t` from point `e` formatted through `n` (xform) at scale `r`.
fn wa(e: V2, t: &Seg, n: &dyn Fn(V2) -> String, r: f64) -> String {
    let Seg::Arc { to, radii, sweep } = t else {
        return String::new();
    };
    if *sweep == 0.0 {
        return String::new();
    }
    if sweep.abs() > JH {
        let s = sweep / 2.0;
        let i = fh2(*radii, e[1].atan2(e[0]) + s);
        return wa(
            e,
            &Seg::Arc {
                to: i,
                radii: *radii,
                sweep: s,
            },
            n,
            r,
        ) + &wa(
            i,
            &Seg::Arc {
                to: *to,
                radii: *radii,
                sweep: s,
            },
            n,
            r,
        );
    }
    format!(
        "A{} {} 0 0 {} {}",
        qt(radii[0] * r),
        qt(radii[1] * r),
        if *sweep > 0.0 { 0 } else { 1 },
        n(*to)
    )
}

/// `ka` — a seg loop → `M..Z` path through projector `t` at scale `n`.
fn ka(e: &[Seg], t: &dyn Fn(V2) -> String, n: f64) -> String {
    let mut r = e[e.len() - 1].to();
    let mut o = format!("M{}", t(r));
    for s in e {
        match s {
            Seg::Line { to } => o += &format!("L{}", t(*to)),
            Seg::Arc { .. } => o += &wa(r, s, t, n),
        }
        r = s.to();
    }
    format!("{}Z", o)
}

/// `DM` — the ellipse ring of `n` points about center `e`, radii `t`.
fn dm(e: V2, t: V2, n: usize) -> Vec<V2> {
    let [r, o] = t;
    (0..n)
        .map(|i| {
            let a = i as f64 / n as f64 * UT;
            let c = a.cos();
            let u = a.sin();
            let l = if r == 0.0 || o == 0.0 {
                0.0
            } else {
                1.0 / (c / r).hypot(u / o)
            };
            [e[0] + c * l, e[1] + u * l]
        })
        .collect()
}

// ---------------------------------------------------------------------------
// the sphere branch (`VT`/`qh`/`jT`/`HT`/`qT`)
// ---------------------------------------------------------------------------

/// `VT` — an eye's projected loops (`Seg` lists) at radius `o`.
fn vt(e: &Eye, t: &EyeFrame, n: bool, r: &M3, o: f64) -> Vec<Vec<Seg>> {
    let s = tt(e, o * e.lid_y.max(1.0));
    if s.is_empty() {
        return vec![];
    }
    let i = et(*r, t.m);
    let a = et(*r, t.c);
    let c = et(*r, t.t);
    let u = i[1].atan2(i[0]);
    let l = e.tilt.cos();
    let d = e.tilt.sin();
    let [f, h] = e.off;
    let p = |[m, y]: V2| -> V3 {
        let w = y * e.lid_y;
        let sv = f + l * m + d * w;
        let b = h - d * m + l * w;
        let v = (sv * sv + b * b).sqrt();
        let ee = if v > 1e-12 { v.sin() / v } else { 1.0 };
        let aa = v.cos();
        [
            aa * i[0] + ee * (sv * a[0] + b * c[0]),
            aa * i[1] + ee * (sv * a[1] + b * c[1]),
            aa * i[2] + ee * (sv * a[2] + b * c[2]),
        ]
    };
    let mut g = Vec::new();
    for m in s.iter() {
        let mut y: Vec<V3> = m.iter().map(|&p2| p(p2)).collect();
        if n {
            y.reverse();
        }
        if let Some(w) = jm(&y, u) {
            g.push(w);
        }
    }
    g
}

/// `qh` — every eye's projected loops for the frame at radius `t`.
fn qh(e: &Frame, t: f64, tab: &Tables) -> Vec<Vec<Seg>> {
    let Some(eyes) = &e.eyes else {
        return vec![];
    };
    let n = cr(e.quat);
    let r = [e.squash[0].abs(), e.squash[1].abs()];
    let o = r[0] < 1.0 || r[1] < 1.0;
    let mut s = Vec::new();
    for (a, i) in eyes.iter().enumerate() {
        for c in vt(i, &tab.eye_frames[a], tab.eye_flip[a], &n, t) {
            let u = if o { et_clip(&c, r) } else { Some(c) };
            if let Some(u) = u {
                s.push(u);
            }
        }
    }
    s
}

/// `jT`'s record.
struct JtRec {
    size: f64,
    body: String,
    eyes: String,
}

/// `jT` — the sphere render record.
fn jt(e: &Frame, t: &PaintOpts, n: f64, tab: &Tables) -> JtRec {
    let o = e.zoom * n / 2.0;
    let [s, i] = e.squash;
    if o == 0.0 || s == 0.0 || i == 0.0 {
        return JtRec {
            size: n,
            body: String::new(),
            eyes: String::new(),
        };
    }
    let a = [s.abs(), i.abs()];
    let c = n / 2.0 + o * e.offset[0];
    let u = n / 2.0 - o * (e.squash_anchor * (1.0 - i) + e.offset[1]);
    let l = |h: V2| format!("{} {}", qt(c + h[0] * o), qt(u - h[1] * o));
    let d = |h: &[Vec<Seg>]| -> String {
        h.iter()
            .map(|p| ka(p, &l, o.abs()))
            .collect::<Vec<_>>()
            .join("")
    };
    let f = qh(e, o.abs(), tab);
    JtRec {
        size: n,
        body: if t.cutout {
            d(&tt2(a, f.clone()))
        } else {
            d(&[qr(a, 1.0)])
        },
        eyes: if t.cutout { String::new() } else { d(&f) },
    }
}

/// `HT` — the sphere branch tree.
fn ht(e: &JtRec, t: &PaintOpts, n: &str, r: f64) -> MarkTree {
    let mut s: Vec<MarkTree> = Vec::new();
    let i = cc(t);
    s.push(r2(
        "path",
        &[("d", e.body.clone()), ("fill", i), ("fill-rule", sv("evenodd"))],
    ));
    if !e.eyes.is_empty() {
        s.push(r2("path", &[("d", e.eyes.clone()), ("fill", uc(t))]));
    }
    kc(e.size, n, r, s)
}

// ---------------------------------------------------------------------------
// `bc`/`qT`/`PM` — the per-body point-ring projections used by `BE`
// ---------------------------------------------------------------------------

/// `qT` — the sphere's ring projection for `bc`.
fn qt2(e: &Frame, t: f64, tab: &Tables) -> ShapeRec {
    let n = e.zoom * t / 2.0;
    let [r, o] = e.squash;
    let s = [
        t / 2.0 + n * e.offset[0],
        t / 2.0 - n * (e.squash_anchor * (1.0 - o) + e.offset[1]),
    ];
    let i = n.abs();
    let a = |[c, u]: V2| -> V2 { [s[0] + c * n, s[1] - u * n] };
    ShapeRec {
        center: s,
        radius: i,
        body: vec![dm(s, [r.abs() * i, o.abs() * i], DT)],
        eyes: qh(e, i, tab)
            .iter()
            .map(|c| bt_pts(c).iter().map(|&p| a(p)).collect())
            .collect(),
    }
}

/// `PM` — the atlas body's ring projection for `bc`.
fn pm(e: &Frame, body: &'static str, t: f64) -> ShapeRec {
    let n = ((t * e.zoom / H_UND).round() as i64).max(8) as f64;
    let r = n.max(IH);
    let o = ch(&e.quat, rh(t, e.zoom));
    let sh = bodies::dollar_h(body, &o, r, e.eyes.as_ref());
    let [a, c, u, l] = oh(e, t, n, r);
    let d = |[f, h]: V2| -> V2 { [a * f + u, c * h + l] };
    ShapeRec {
        center: [a * r / 2.0 + u, c * r / 2.0 + l],
        radius: e.zoom * t / 2.0,
        body: sh
            .body
            .iter()
            .flat_map(|f| ad(f))
            .map(|f| f.iter().map(|&p| d(p)).collect())
            .collect(),
        eyes: if e.eyes.is_none() {
            vec![]
        } else {
            sh.eyes
                .iter()
                .flat_map(|f| ad(f))
                .map(|f| f.iter().map(|&p| d(p)).collect())
                .collect()
        },
    }
}

/// `bc`'s shape record.
struct ShapeRec {
    center: V2,
    radius: f64,
    body: Vec<Vec<V2>>,
    eyes: Vec<Vec<V2>>,
}

/// `bc` — `sphere` → `qT`, else `PM` (the eye/body ring source of `BE`).
fn bc(e: &Frame, t: &'static str, n: f64, tab: &Tables) -> ShapeRec {
    if t == "sphere" {
        qt2(e, n, tab)
    } else {
        pm(e, t, n)
    }
}

// ---------------------------------------------------------------------------
// polar outline blending (`OM`/`LM`/`NM`/`UM`/`KM`/`fs`/`XM`/`YM`/`ZM`/`WM`/
// `FM`/`zM`/`jM`/`qM`/`BM`/`HM`/`Lh`/`VM`/`GM`/`Nh`/`KE`/`XE`)
// ---------------------------------------------------------------------------

fn om(e: usize) -> Vec<f64> {
    (0..e).map(|n| n as f64 / e as f64 * UT).collect()
}

/// `fs` — the [0,2π) angle of point (t,n) about `e`.
fn fs(e: V2, t: f64, n: f64) -> f64 {
    let r = (n - e[1]).atan2(t - e[0]);
    if r < 0.0 {
        r + UT
    } else {
        r
    }
}

/// `LM` — the angle of every outline point about `t`.
fn lm(e: &[Vec<V2>], t: V2) -> Vec<f64> {
    e.iter()
        .flat_map(|n| n.iter().map(move |&[r, o]| fs(t, r, o)))
        .collect()
}

/// `NM` — sorted dedup of angle lists.
fn nm(e: Vec<Vec<f64>>) -> Vec<f64> {
    let mut t: Vec<f64> = e.into_iter().flatten().collect();
    t.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    t.iter()
        .enumerate()
        .filter_map(|(r, &n)| {
            if r == 0 || n - t[r - 1] > RM {
                Some(n)
            } else {
                None
            }
        })
        .collect()
}

/// `UM` — edge buckets of the outline `e` by angle about `t`.
fn um(e: &[Vec<V2>], t: V2) -> Vec<Vec<(V2, V2)>> {
    let mut n: Vec<Vec<(V2, V2)>> = vec![Vec::new(); TN];
    for r in e {
        for o in 0..r.len() {
            let s = r[o];
            let i = r[(o + 1) % r.len()];
            let a = fs(t, s[0], s[1]);
            let mut c = fs(t, i[0], i[1]) - a;
            if c > PI {
                c -= UT;
            }
            if c < -PI {
                c += UT;
            }
            let u = (((if c < 0.0 { a + c } else { a }) / UT * TN as f64).floor() as i64) - 1;
            let l = (((if c < 0.0 { a } else { a + c }) / UT * TN as f64).floor() as i64) + 1;
            let mut d = u;
            while d <= l {
                n[(((d % TN as i64) + TN as i64) % TN as i64) as usize].push((s, i));
                d += 1;
            }
        }
    }
    n
}

/// `KM` — ray reach of outline `e` at angle `r` about (t,n).
fn km(e: &[Vec<(V2, V2)>], t: V2, r: f64) -> f64 {
    let o = r.cos();
    let s = r.sin();
    let mut i = f64::NAN;
    for &(a, u) in &e[(r / UT * TN as f64).floor() as usize % TN] {
        let d = u[0] - a[0];
        let f = u[1] - a[1];
        let h = o * f - s * d;
        if h.abs() < 1e-12 {
            continue;
        }
        let p = a[0] - t[0];
        let g = a[1] - t[1];
        let m = (p * f - g * d) / h;
        let y = (p * s - g * o) / h;
        if m >= 0.0 && (-CD..=1.0 + CD).contains(&y) && !(m <= i) {
            i = m;
        }
    }
    i
}

/// `XM` — fill NaN reaches by neighbor interpolation.
fn xm(e: &[f64]) -> Vec<f64> {
    let t = e.len();
    let n: Vec<usize> = e
        .iter()
        .enumerate()
        .filter_map(|(o, r)| if r.is_nan() { None } else { Some(o) })
        .collect();
    if n.is_empty() {
        return e.iter().map(|_| 0.0).collect();
    }
    e.iter()
        .enumerate()
        .map(|(o, &r)| {
            if !r.is_nan() {
                return r;
            }
            let s = n.iter().copied().find(|&c| c > o).unwrap_or(n[0] + t) as i64;
            let i = n
                .iter()
                .copied()
                .rev()
                .find(|&c| c < o)
                .map(|c| c as i64)
                .unwrap_or_else(|| n[n.len() - 1] as i64 - t as i64);
            let a = (o as i64 - i) as f64 / (s - i) as f64;
            e[(((i + t as i64) % t as i64 + t as i64) % t as i64) as usize] * (1.0 - a)
                + e[(s as usize) % t] * a
        })
        .collect()
}

/// `FM` — resample outline `e` about center `t` at angles `n`.
fn fm(e: &[Vec<V2>], t: V2, n: &[f64]) -> Vec<V2> {
    let r = um(e, t);
    let raw: Vec<f64> = n.iter().map(|&s| km(&r, t, s)).collect();
    let o = xm(&raw);
    n.iter()
        .enumerate()
        .map(|(i, &s)| [t[0] + s.cos() * o[i], t[1] + s.sin() * o[i]])
        .collect()
}

/// `YM` — interpolated reach at angle `t`.
fn ym(e: &[(f64, f64)], t: f64) -> f64 {
    let n = e.len();
    if n == 0 {
        return 0.0;
    }
    let mut r = e.iter().position(|c| c.0 >= t).map(|i| i as i64).unwrap_or(-1);
    if r < 0 {
        r = 0;
    }
    let r = r as usize;
    let o = e[r];
    let s = e[(r + n - 1) % n];
    let i = (o.0 - s.0 + UT) % UT;
    if i == 0.0 {
        return o.1;
    }
    let a = (t - s.0 + UT) % UT / i;
    s.1 + (o.1 - s.1) * a
}

/// `zM` — resample pt ring `e` about `t` at angles `n` (by reach interp).
fn zm(e: &[V2], t: V2, n: &[f64]) -> Vec<V2> {
    let mut r: Vec<(f64, f64)> = e
        .iter()
        .map(|&[o, s]| (fs(t, o, s), (o - t[0]).hypot(s - t[1])))
        .collect();
    r.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    n.iter()
        .map(|&o| {
            let s = ym(&r, o);
            [t[0] + o.cos() * s, t[1] + o.sin() * s]
        })
        .collect()
}

/// `WM` — signed area of ring `e`.
fn wm(e: &[V2]) -> f64 {
    let mut t = 0.0;
    for n in 0..e.len() {
        let [r, o] = e[n];
        let [s, i] = e[(n + 1) % e.len()];
        t += r * i - s * o;
    }
    t / 2.0
}

/// `Nh` — the centroid of a loop.
fn nh(e: &[V2]) -> V2 {
    if e.is_empty() {
        return [0.0, 0.0];
    }
    let (mut t, mut n) = (0.0, 0.0);
    for &[r, o] in e {
        t += r;
        n += o;
    }
    [t / e.len() as f64, n / e.len() as f64]
}

/// `ZM` — the rightmost crossing at `t` of ring `e` (for winding fix).
fn zm2(e: &[V2], t: V2) -> Option<(usize, V2)> {
    let mut r: Option<(usize, V2)> = None;
    for o in 0..e.len() {
        let [s, i] = e[o];
        let [a, c] = e[(o + 1) % e.len()];
        if i == c || (i - t[1]) * (c - t[1]) > 0.0 {
            continue;
        }
        let u = s + (t[1] - i) / (c - i) * (a - s);
        if u >= t[0] && r.as_ref().is_none_or(|(_, p)| u > p[0]) {
            r = Some((o, [u, t[1]]));
        }
    }
    r
}

/// `jM`'s record: a wound ring with arc-length stations.
struct JmRec {
    points: Vec<V2>,
    stations: Vec<f64>,
}

/// `jM` — normalize a loop's winding and station it.
fn jm2(e: &[V2]) -> JmRec {
    let t = if wm(e) < 0.0 {
        e.iter().rev().copied().collect::<Vec<_>>()
    } else {
        e.to_vec()
    };
    let c = nh(&t);
    let n = zm2(&t, c);
    let r: Vec<V2> = match n {
        None => t.clone(),
        Some((edge, point)) => {
            let mut v = vec![point];
            v.extend(t[edge + 1..].iter().copied());
            v.extend(t[..edge + 1].iter().copied());
            v
        }
    };
    let mut o = vec![0.0f64];
    for i in 0..r.len() {
        let [a, c2] = r[i];
        let [u, l] = r[(i + 1) % r.len()];
        o.push(o[i] + (u - a).hypot(l - c2));
    }
    let s = *o.last().unwrap_or(&0.0);
    JmRec {
        points: r,
        stations: o.iter().map(|&i| if s == 0.0 { 0.0 } else { i / s }).collect(),
    }
}

/// `qM` — sample a `jM` ring at stations `n`.
fn qm(e: &JmRec, n: &[f64]) -> Vec<V2> {
    if e.points.is_empty() {
        return vec![];
    }
    let mut r = 0usize;
    n.iter()
        .map(|&o| {
            while r < e.points.len() - 1 && e.stations[r + 1] < o {
                r += 1;
            }
            let [s, i] = e.points[r];
            let [a, c] = e.points[(r + 1) % e.points.len()];
            let u = e.stations[r + 1] - e.stations[r];
            let l = if u == 0.0 { 0.0 } else { (o - e.stations[r]) / u };
            [s + (a - s) * l, i + (c - i) * l]
        })
        .collect()
}

/// `BM` — `n` even stations.
fn bm(e: usize) -> Vec<f64> {
    (0..e).map(|n| n as f64 / e as f64).collect()
}

/// `HM` — merged deduped station list.
fn hm(e: Vec<Vec<f64>>) -> Vec<f64> {
    let mut t: Vec<f64> = e.into_iter().flatten().filter(|&n| n < 1.0).collect();
    t.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    t.iter()
        .enumerate()
        .filter_map(|(r, &n)| {
            if r == 0 || n - t[r - 1] > M_DOLLAR {
                Some(n)
            } else {
                None
            }
        })
        .collect()
}

/// `Lh` — the weighted outline blend.
fn lh(e: &[(Vec<V2>, f64)]) -> Vec<V2> {
    let t: f64 = e.iter().map(|(_, w)| w).sum();
    let Some((n, _)) = e.first() else {
        return vec![];
    };
    if t == 0.0 {
        return vec![];
    }
    n.iter()
        .enumerate()
        .map(|(o, _)| {
            let (mut s, mut i) = (0.0, 0.0);
            for (a, c) in e {
                s += a[o][0] * c;
                i += a[o][1] * c;
            }
            [s / t, i / t]
        })
        .collect()
}

/// `VM`/`mv` — lerp ring `e` toward `t` by `n`.
fn vm(e: &[V2], t: &[V2], n: f64) -> Vec<V2> {
    e.iter()
        .enumerate()
        .map(|(s, &[r, o])| [r + (t[s][0] - r) * n, o + (t[s][1] - o) * n])
        .collect()
}

/// `GM` — a constant ring of `t` points at `e`.
fn gm(e: V2, t: usize) -> Vec<V2> {
    (0..t).map(|_| e).collect()
}

/// `XE` — the nearest unclaimed anchor slot.
fn xe(e: &[KeSlot], t: usize, n: V2, r: f64) -> Option<usize> {
    let mut o: Option<usize> = None;
    let mut s = r;
    for (idx, i) in e.iter().enumerate() {
        if i.loops[t].is_some() {
            continue;
        }
        let a = (i.anchor[0] - n[0]).hypot(i.anchor[1] - n[1]);
        if a < s {
            o = Some(idx);
            s = a;
        }
    }
    o
}

struct KeSlot {
    anchor: V2,
    loops: Vec<Option<Vec<V2>>>,
}

/// `KE` — eye loops blended across body shapes by anchor + station.
fn ke(e: &[(ShapeRec, f64)]) -> Vec<Vec<V2>> {
    let t = e.first().map(|(r, _)| r.radius).unwrap_or(0.0) * QE;
    let mut n: Vec<KeSlot> = Vec::new();
    for (o, (r, _)) in e.iter().enumerate() {
        for s in &r.eyes {
            let i = nh(s);
            if let Some(a) = xe(&n, o, i, t) {
                n[a].loops[o] = Some(s.clone());
                continue;
            }
            let mut c: Vec<Option<Vec<V2>>> = e.iter().map(|_| None).collect();
            c[o] = Some(s.clone());
            n.push(KeSlot {
                anchor: i,
                loops: c,
            });
        }
    }
    n.iter()
        .map(|r| {
            let o: Vec<Option<JmRec>> = r
                .loops
                .iter()
                .map(|i| i.as_ref().map(|i| jm2(i)))
                .collect();
            let s = hm(vec![bm(96)]
                .into_iter()
                .chain(o.iter().filter_map(|i| i.as_ref().map(|i| i.stations.clone())))
                .collect());
            lh(&e
                .iter()
                .enumerate()
                .map(|(a, (_, i))| {
                    let c = &o[a];
                    (
                        match c {
                            None => gm(r.anchor, s.len()),
                            Some(c) => qm(c, &s),
                        },
                        *i,
                    )
                })
                .collect::<Vec<_>>())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// the morph parts (`Ht`/`$s`/`Vh`/`gE`/`TE`/`EE`/`PE`/`AE`/`IE`/`_E`/`CE`/
// `RE`/`$E`/`OE`/`LE`/`NE`/`FE`) and driver (`DE`/`Gh`/`kE`/`wE`/`bE`/`xE`/
// `wc`/`Wh`/`vE`/`SE`/`ME`/`Ai`/`lE`/`GE`/`WE`)
// ---------------------------------------------------------------------------

/// A `FE` part (`{type,key,...}` — `key` dropped, paint keys off position).
#[derive(Clone)]
pub(crate) enum Part {
    Disc { cx: f64, cy: f64, r: f64, opacity: f64 },
    Ring { cx: f64, cy: f64, r: f64, width: f64, opacity: f64, arc: Option<(f64, f64)> },
    Shape { d: String, transform: String, opacity: f64 },
    Stroke { d: String, width: f64, opacity: f64 },
}

impl Part {
    fn opacity(&self) -> f64 {
        match self {
            Part::Disc { opacity, .. }
            | Part::Ring { opacity, .. }
            | Part::Shape { opacity, .. }
            | Part::Stroke { opacity, .. } => *opacity,
        }
    }
}

fn ht_disc(t: f64, n: f64, r: f64, o: f64) -> Part {
    Part::Disc {
        cx: t,
        cy: n,
        r,
        opacity: o,
    }
}
fn dollar_s(t: f64, n: f64, r: f64) -> Part {
    Part::Ring {
        cx: X,
        cy: X,
        r: t,
        width: n,
        opacity: r,
        arc: None,
    }
}
fn vh(t: String, n: String, r: f64) -> Part {
    Part::Shape {
        d: t,
        transform: n,
        opacity: r,
    }
}
fn ge(t: String, n: f64, r: f64) -> Part {
    Part::Stroke {
        d: t,
        width: n,
        opacity: r,
    }
}

/// `yE` — the dots defaults.
struct Ye {
    lift: f64,
    pop: f64,
    tone: f64,
}
const YE: Ye = Ye {
    lift: 0.0,
    pop: 1.0,
    tone: 1.0,
};

/// `Gh` — the dots' bounce envelope.
fn gh(e: (f64, f64), t: f64, n: f64, r: bool) -> Ye {
    let o = bt(e.0 * 1e3 / OE2 + 0.119);
    let s = (o - t / 3.0).abs();
    let i = s.min(1.0 - s);
    let a = if r {
        1.0
    } else {
        (-(i * i) / (2.0 * 0.15 * 0.15)).exp()
    };
    let c = if r { 0.0 } else { 1.0 };
    Ye {
        lift: a * RE2 * n * c,
        pop: 1.0 + c * (EE2 + TE2 * a - 1.0),
        tone: 1.0 - c * 0.5 * (1.0 - a),
    }
}

/// `kE` — the wave's base envelope.
fn ke2(e: f64) -> f64 {
    0.42 + 0.29 * (e * 0.0021).sin() * (e * 0.0034).sin()
        + 0.29 * (e * 0.0013 + 1.7).sin()
}

/// `wE` — the wave's per-column envelope.
fn we(e: f64, t: f64) -> f64 {
    ke2(e) * (0.55 + 0.45 * (e * 0.012 - t.abs() * 1.05).sin())
}

/// `bE` — the receive angle for slot `e`.
fn be2(e: f64) -> f64 {
    let mut f = ic(e);
    -PI * 1.25 + f() * PI * 1.5
}

/// `xE` — the ball's bounce curve.
fn xe2(e: f64) -> f64 {
    let r = 1082.2060353798126f64;
    let o = 40.0f64;
    let s = (2.0 * o / r).sqrt();
    if e < s {
        return 0.5 * r * e * e;
    }
    let i = bt((e - s) / 0.62);
    o - 208.0 * i * (1.0 - i)
}

/// `wc`'s record.
struct Wc {
    x: f64,
    y: f64,
    wig: f64,
    rot: f64,
}

/// `wc` — the pencil's travel pose at ms `e`.
fn wc(e: f64) -> Wc {
    let t = bt(e / PS);
    if t < PR {
        let r = t / PR;
        let o = r * r * (3.0 - 2.0 * r);
        let s = le(r / 0.08, 0.0, 1.0) * le((1.0 - r) / 0.08, 0.0, 1.0);
        return Wc {
            x: -54.0 + 118.0 * o,
            y: 26.0,
            wig: (r * 24.0).sin() * 3.2 * s,
            rot: 17.0 + (e * 6e-4).sin() * 1.0,
        };
    }
    let n = ts((t - PR) / (1.0 - PR));
    Wc {
        x: 64.0 - 118.0 * n,
        y: 26.0 - 20.0 * (n * PI).sin(),
        wig: 0.0,
        rot: 17.0 - 2.0 * (n * PI).sin() + (e * 6e-4).sin() * 1.0,
    }
}

/// `Wh` — the pencil's tip point at ms `e` (in the 228.5 box).
pub(crate) fn wh(e: f64) -> V2 {
    let t = wc(e);
    [X + t.x, X + t.y + t.wig + 19.0]
}

/// `SE` — the live trail points at ms `e` (≤64 tail).
fn se(e: f64, tab: &Tables) -> Vec<V2> {
    let t = bt(e / PS) * PS;
    let n = t.min(OO);
    let r: Vec<&(f64, V2)> = tab.pencil_trail.iter().filter(|a| a.0 <= n).collect();
    let o = r.last().map(|a| a.0).unwrap_or(n);
    let s: Vec<V2> = if n > o { vec![wh(n)] } else { vec![] };
    let mut i: Vec<V2> = r.iter().map(|a| a.1).collect();
    i.extend(s);
    let i: Vec<V2> = i.iter().rev().take(64).rev().copied().collect();
    if t < OO {
        i
    } else {
        i.iter()
            .skip(((t - OO) / 1e3 * IE2) as usize)
            .copied()
            .collect()
    }
}

/// `ME` — Catmull-Rom through `e` (`.toFixed(1)` coords).
fn me_path(e: &[V2]) -> String {
    let t = e.len();
    if t == 0 {
        return String::new();
    }
    let mut n = format!("M{:.1} {:.1}", e[0][0], e[0][1]);
    if t == 2 {
        return n + &format!("L{:.1} {:.1}", e[1][0], e[1][1]);
    }
    for r in 0..t - 1 {
        let o = e[r.saturating_sub(1)];
        let s = e[r];
        let i = e[r + 1];
        let a = e[(r + 2).min(t - 1)];
        let c = s[0] + (i[0] - o[0]) / 6.0;
        let u = s[1] + (i[1] - o[1]) / 6.0;
        let l = i[0] - (a[0] - s[0]) / 6.0;
        let d = i[1] - (a[1] - s[1]) / 6.0;
        n += &format!("C{:.1} {:.1} {:.1} {:.1} {:.1} {:.1}", c, u, l, d, i[0], i[1]);
    }
    n
}

/// `TE` — dots.
fn te2(e: f64, t: (f64, f64), n: bool) -> Vec<Part> {
    let r = [X - VD, X + VD];
    let mut o = Vec::new();
    for (s, &position) in r.iter().enumerate() {
        let i = le((e - s as f64 * 0.12) / (1.0 - s as f64 * 0.12), 0.0, 1.0);
        if i <= 0.004 {
            continue;
        }
        let a = ye(i);
        let c = ts(i);
        let u = gh(t, if s == 0 { 0.0 } else { 2.0 }, e, n);
        o.push(ht_disc(
            X + (position - X) * c,
            X - u.lift,
            JT3 * a * u.pop * NE2,
            a * u.tone,
        ));
    }
    o
}

/// `EE` — orbit.
fn ee2(e: f64, t: (f64, f64)) -> Vec<Part> {
    let n = ye(e);
    let r = 52.0 * ts(e);
    let o = 12.0;
    let s = t.0 * 1e3 * 0.0017;
    (0..5)
        .map(|a| {
            let c = s + a as f64 * TAU / 5.0;
            let u = c.cos();
            let l = 0.5 + 0.5 * le(u, 0.0, 1.0);
            ht_disc(
                X + r * c.sin(),
                X - r * 0.42 * c.cos(),
                (o * l * n).max(0.3),
                le((u + 0.4) / 0.6, 0.18, 1.0) * n,
            )
        })
        .collect()
}

/// `PE` — radar.
fn pe(e: f64, t: (f64, f64), n: f64) -> Vec<Part> {
    let r = ye(e);
    let o = 1300.0;
    let s = 104.0;
    (0..3)
        .map(|a| {
            let c = bt(t.0 * 1e3 / o + a as f64 / 3.0);
            dollar_s(
                n + (s - n) * c,
                3.4 * (1.0 - c * 0.55),
                r * (1.0 - c) * 0.9,
            )
        })
        .collect()
}

/// `AE` — progress.
fn ae(e: f64, t: (f64, f64)) -> Vec<Part> {
    let n = ye(e);
    let r = 62.0 * ts(e);
    let o = le(t.1 * 1e3 / ae2(), 0.0, 1.0);
    let s = le(o / 0.85, 0.0, 1.0);
    let i = dollar_s(r, 5.0, n * 0.16);
    if s <= 0.0 {
        vec![i]
    } else {
        vec![
            i,
            Part::Ring {
                cx: X,
                cy: X,
                r,
                width: 5.0,
                opacity: n,
                arc: Some((s, -90.0)),
            },
        ]
    }
}

/// `IE` — gather.
fn ie2(e: f64, t: (f64, f64)) -> Vec<Part> {
    let n = ye(e);
    let mut r = Vec::new();
    for o in 0..5usize {
        let s = le((t.1 * 1e3 / ce2() - o as f64 * 0.09) / 0.62, 0.0, 1.0);
        if s >= 1.0 {
            continue;
        }
        let i = 1.0 - (1.0 - s).powi(3);
        let a = o as f64 * 2.4 + s * 2.2;
        let c = 96.0 * (1.0 - i);
        r.push(ht_disc(
            X + c * a.cos(),
            X + c * a.sin() * 0.8,
            9.0 * (0.5 + 0.5 * i) * n,
            n * le(s * 5.0, 0.0, 1.0) * (1.0 - i * 0.25),
        ));
    }
    r
}

/// `_E` — wave.
fn _e(e: f64, t: (f64, f64)) -> Vec<Part> {
    let n = t.0 * 1e3;
    let r = 44.0;
    let mut o = Vec::new();
    for s in UE_ARR {
        let i = le((e - s.abs() * 0.1) / (1.0 - s.abs() * 0.1), 0.0, 1.0);
        if i <= 0.004 {
            continue;
        }
        let a = ts(i);
        let c = we(n, s);
        let u = (7.0 + 9.0 * le(c, 0.08, 1.0)) * ye(i);
        let l = 6.0 * le(c, 0.0, 1.0) * i;
        o.push(ht_disc(X + s * r * a, X - l, u, i));
    }
    o
}

/// `CE` — send.
fn ce(e: f64, t: (f64, f64)) -> Vec<Part> {
    let n = ye(e);
    let r = bt(t.0 * 1e3 / BH);
    let o = le((r - 0.18) / 0.55, 0.0, 1.0);
    let s = o * o * (0.4 + 0.6 * o);
    let i = 0.74;
    let a = -0.62;
    let mut c = Vec::new();
    if o > 0.0 && o < 1.0 {
        let f = 108.0 * s;
        c.push(ht_disc(
            X + i * f,
            X + a * f,
            10.0 * (1.0 - s * 0.55) * n,
            n * (1.0 - s * s),
        ));
    }
    let u = le((r - 0.26) / 0.55, 0.0, 1.0);
    let l = u * u * (0.4 + 0.6 * u);
    if o > 0.0 && u > 0.0 && u < 1.0 {
        let f = 108.0 * l;
        c.push(ht_disc(
            X + i * f,
            X + a * f,
            5.0 * (1.0 - l * 0.6) * n,
            n * 0.3 * (1.0 - l),
        ));
    }
    let d = le((r - 0.18) / 0.3, 0.0, 1.0);
    if d > 0.0 && d < 1.0 {
        c.push(dollar_s(
            20.0 + 34.0 * ye(d),
            2.8 * (1.0 - d),
            n * (1.0 - d) * 0.8,
        ));
    }
    c
}

/// `RE` — receive.
fn re2(e: f64, t: (f64, f64)) -> Vec<Part> {
    let n = ye(e);
    let r = t.0 * 1e3;
    let o = be2((r / XA).floor());
    let s = bt(r / XA);
    let i = le(s / 0.6, 0.0, 1.0);
    let a = 1.0 - (1.0 - i).powi(3);
    let c = o.cos();
    let u = o.sin();
    let l = 108.0 * (1.0 - a);
    let mut d = Vec::new();
    if i < 1.0 {
        let h = 18.0 * (i * PI).sin() * (1.0 - a * 0.7);
        d.push(ht_disc(
            X + c * l - u * h,
            X + u * l + c * h,
            3.5 + 6.5 * a,
            n * le(i * 3.5, 0.0, 1.0) * (0.3 + 0.7 * a),
        ));
    }
    let f = le((s - 0.58) / 0.32, 0.0, 1.0);
    if f > 0.0 && f < 1.0 {
        d.push(dollar_s(
            20.0 + 26.0 * ye(f),
            2.8 * (1.0 - f),
            n * (1.0 - f) * 0.8,
        ));
    }
    d
}

/// `$E` — dock.
fn dollar_e(e: f64, t: (f64, f64)) -> Vec<Part> {
    let n = ye(e);
    let r = t.0 * 1e3;
    let o = 42.0;
    let s = 1.1;
    let mut i = Vec::new();
    for a in 0..2usize {
        let c = le((t.0 - (0.2 + a as f64 * 1.3)) / 0.9, 0.0, 1.0);
        if c <= 0.0 {
            continue;
        }
        let u = 1.0 - (1.0 - c).powi(3);
        let l = r * 0.001 * s + a as f64 * PI;
        let d = X + o * l.sin();
        let f = X + o * 0.5 * l.cos() + (r * 0.003 + a as f64).sin() * 2.0;
        let h = X - 120.0 + a as f64 * 30.0;
        let p = X + 95.0;
        i.push(ht_disc(
            h + (d - h) * u,
            p + (f - p) * u,
            (7.0 + 3.0 * u) * n,
            n * le(c * 4.0, 0.0, 1.0),
        ));
    }
    i
}

/// `OE` — pencil.
fn oe(e: f64, t: (f64, f64), tab: &Tables) -> Vec<Part> {
    let n = t.0 * 1e3;
    let r = wc(n);
    let o = (r.rot - 90.0) * PI / 180.0;
    let s = 68.0;
    let i = o.cos() * s;
    let a = o.sin() * s;
    let c = X + (r.x + i) * e;
    let u = X + (r.y + r.wig * 0.15 + a) * e;
    let mut l = vec![vh(
        tab.pencil_shape.clone(),
        format!(
            "translate({:.1} {:.1}) rotate({:.1}) scale({:.3}) translate({} {})",
            c,
            u,
            r.rot * e,
            ye(e),
            -X,
            -X
        ),
        le(e * 1.6 - 0.3, 0.0, 1.0),
    )];
    let d = se(n, tab);
    if d.len() >= 2 {
        l.push(ge(me_path(&d), 6.0, le(e * 1.2, 0.0, 1.0)));
    }
    l
}

/// `LE` — bang.
fn le2(e: f64, t: (f64, f64), tab: &Tables) -> Vec<Part> {
    let n = t.0;
    let r = ye(le(e * 1.1, 0.0, 1.0));
    let o = (-((n % 2.2) * 5.5)).exp();
    let s = (n * 42.0).sin() * 2.2 * o;
    vec![vh(
        tab.bang_shape.clone(),
        format!(
            "translate(0 {:.1}) rotate({:.2} {} {:.1}) translate({} {}) scale({:.3}) translate({} {})",
            -26.0 - (1.0 - r) * 70.0,
            s,
            X,
            X - 74.0,
            X,
            X,
            le(e * 1.2, 0.0, 1.0),
            -X,
            -X
        ),
        le(e * 1.5 - 0.2, 0.0, 1.0),
    )]
}

/// `NE` — standby.
fn ne(e: f64, t: (f64, f64)) -> Vec<Part> {
    let n = 0.5 + 0.5 * (t.0 * 1e3 * 0.0016).sin();
    vec![ht_disc(X, X, 26.0 + 7.0 * n, ye(e) * (0.06 + 0.1 * n))]
}

/// `FE` — the parts of morph kind `e` at weight `t`, clock `n`, radius `r`.
fn fe(e: &str, t: f64, n: (f64, f64), r: f64, o: bool, tab: &Tables) -> Vec<Part> {
    let _ = t;
    match e {
        "dots" => te2(t, n, o),
        "orbit" => ee2(t, n),
        "radar" => pe(t, n, r),
        "progress" => ae(t, n),
        "gather" => ie2(t, n),
        "wave" => _e(t, n),
        "send" => ce(t, n),
        "receive" => re2(t, n),
        "dock" => dollar_e(t, n),
        "ball" => vec![],
        "pencil" => oe(t, n, tab),
        "bang" => le2(t, n, tab),
        "standby" => ne(t, n),
        _ => vec![],
    }
}

/// `lE` — rotate a ring's indexing by `t` and each pt by `t/len*2π`.
pub(crate) fn le_ring(e: &[V2], t: usize) -> Vec<V2> {
    let n = e.len();
    let r = t as f64 / n as f64 * TAU;
    let o = r.cos();
    let s = r.sin();
    (0..n)
        .map(|a| {
            let [c, u] = e[((a + n - t) % n + n) % n];
            let l = c - X;
            let d = u - X;
            [X + l * o - d * s, X + l * s + d * o]
        })
        .collect()
}

/// `Ai` — the kind's travel ring.
fn ai<'a>(e: &str, tab: &'a Tables) -> &'a [V2] {
    if e == "pencil" {
        &tab.pencil_ring
    } else {
        &tab.circle_ring
    }
}

/// `DE`'s core record.
struct DeCore {
    scale: f64,
    dx: f64,
    dy: f64,
    rotation: f64,
    opacity: f64,
    round: f64,
    ring: Vec<V2>,
}

struct De {
    core: DeCore,
    zoom: f64,
    parts: Vec<Part>,
}

/// `DE` — the morph driver record.
fn de(e: &MorphSnap, t: f64, tab: &Tables) -> De {
    let n = e.kind;
    let r = e.is_reduced_motion;
    let o = le(e.amount, 0.0, 1.0);
    let s = le(e.swap, 0.0, 1.0);
    let i = if s < 0.999 { e.outgoing } else { None };
    let a = |j: Option<&str>| -> f64 {
        if j == n {
            o * s
        } else if j.is_some() && j == i {
            o * (1.0 - s)
        } else {
            0.0
        }
    };
    let c = |j: Option<&str>| -> (f64, f64) {
        if j == n {
            e.clock
        } else {
            e.outgoing_clock
        }
    };
    let u = le(o / ZT_M, 0.0, 1.0);
    let d: Vec<V2> = if n == Some("pencil") || i == Some("pencil") {
        if s >= 0.999 {
            ai(n.unwrap_or(""), tab).to_vec()
        } else {
            vm(
                ai(i.unwrap_or(""), tab),
                ai(n.unwrap_or(""), tab),
                ts(s),
            )
        }
    } else {
        tab.circle_ring.clone()
    };
    let f = n.map_or(19.0, |n| pi(n) * s + i.map_or(pi(n), pi) * (1.0 - s));
    let h = a(Some("dots"));
    let p = n == Some("dots") || i == Some("dots");
    let g = if p {
        gh(c(Some("dots")), 1.0, o, r)
    } else {
        Ye {
            lift: YE.lift,
            pop: YE.pop,
            tone: YE.tone,
        }
    };
    let mut m = if p {
        1.0 + (g.pop - 1.0) * (h / o.max(0.001))
    } else {
        1.0
    };
    let y = a(Some("receive"));
    if y > 0.004 {
        let j = bt(c(Some("receive")).0 * 1e3 / XA);
        let jj = le((j - 0.58) / 0.34, 0.0, 1.0);
        m *= 1.0 + 0.11 * (jj * PI).sin() * y;
    }
    let w = a(Some("send"));
    if w > 0.004 {
        let j = bt(c(Some("send")).0 * 1e3 / BH);
        let jj = if j < 0.18 {
            -0.06 * (j / 0.18 * PI).sin()
        } else {
            0.0
        };
        let f2 = if (0.18..0.42).contains(&j) {
            0.05 * ((j - 0.18) / 0.24 * PI).sin()
        } else {
            0.0
        };
        m *= 1.0 + (jj + f2) * w;
    }
    let ss = a(Some("bang"));
    if ss > 0.004 {
        m *= 1.0 + 0.04 * (-((c(Some("bang")).0 % 2.2) * 5.5)).exp() * ss;
    }
    let (mut b, mut v, mut ee) = (0.0, 0.0, 0.0);
    let aa = a(Some("pencil"));
    if aa > 0.004 {
        let j = wc(c(Some("pencil")).0 * 1e3);
        b += j.x * aa;
        v += (j.y + j.wig * 0.5) * aa;
        ee += j.rot * aa;
    }
    if ss > 0.004 {
        v += 58.0 * ss;
    }
    let ff = a(Some("ball"));
    if ff > 0.004 {
        v += xe2(c(Some("ball")).0) * ff;
    }
    let nn = f / X * m;
    let rr = a(Some("standby"));
    let bb = if rr > 0.0 {
        (0.28 + 0.2 * (c(Some("standby")).0 * 1e3 * 0.0016).sin()) * rr
    } else {
        0.0
    };
    let core = DeCore {
        scale: 1.0 - o + nn * o,
        dx: b * o,
        dy: -g.lift * h + v * o,
        rotation: ee * o,
        opacity: (1.0 - (1.0 - g.tone) * h) * (1.0 - bb),
        round: ts(u),
        ring: d,
    };
    let ii = 1.0 - wb(le((t - 44.0) / 90.0, 0.0, 1.0));
    let under = n.map_or(1.0, xd);
    let oo = i.map_or(under, xd);
    let hh = 1.0 + (under * s + oo * (1.0 - s) - 1.0) * o * ii;
    let mut l: Vec<Part> = Vec::new();
    for j in [i, n] {
        let Some(j) = j else { continue };
        let jj = a(Some(j));
        if jj > 0.004 {
            l.extend(fe(j, jj, c(Some(j)), f, r, tab));
        }
    }
    let _ = oo;
    De {
        core,
        zoom: hh,
        parts: l.into_iter().filter(|j| j.opacity() > 0.0).collect(),
    }
}

/// `GE` — the morph ring projected about center `t` at radius `n`, at `r`.
fn ge2(e: &De, t: V2, n: f64, r: &[f64]) -> Vec<V2> {
    let o = n / X;
    let s: Vec<V2> = e
        .core
        .ring
        .iter()
        .map(|&[i, a]| [t[0] + (i - X) * o, t[1] + (a - X) * o])
        .collect();
    zm(&s, t, r)
}

/// `WE` — the head transform for the morph.
struct We {
    translate: V2,
    rotation: f64,
    scale: f64,
    opacity: f64,
}

fn we2(e: &De, t: f64, n: f64, r: f64) -> We {
    let o = &e.core;
    let s = 1.0 - t;
    let i = if t > 0.0 { (o.scale - s) / t } else { 0.0 };
    We {
        translate: [o.dx * r, o.dy * r],
        rotation: o.rotation,
        scale: s + i * (ON / n) * t,
        opacity: o.opacity,
    }
}

/// `UE` — one part → a node.
fn ue(e: &Part, t: &str) -> MarkTree {
    let n: Vec<(&'static str, String)> = if e.opacity() < 1.0 {
        vec![("opacity", me(e.opacity(), 3))]
    } else {
        vec![]
    };
    match e {
        Part::Disc { cx, cy, r, .. } => {
            let mut a = vec![
                ("cx", me(*cx, 2)),
                ("cy", me(*cy, 2)),
                ("r", me(*r, 2)),
                ("fill", sv(t)),
            ];
            a.extend(n);
            r2("circle", &a)
        }
        Part::Ring {
            cx, cy, r, width, arc, ..
        } => {
            let rr = TAU * r;
            let mut a = vec![
                ("cx", me(*cx, 2)),
                ("cy", me(*cy, 2)),
                ("r", me(*r, 2)),
                ("fill", sv("none")),
                ("stroke", sv(t)),
                ("stroke-width", me(*width, 2)),
            ];
            if let Some((fraction, start_deg)) = arc {
                a.push(("stroke-dasharray", format!("{} {}", me(rr * fraction, 2), me(rr, 2))));
                a.push((
                    "transform",
                    format!("rotate({} {} {})", me(*start_deg, 2), me(*cx, 2), me(*cy, 2)),
                ));
            }
            a.extend(n);
            r2("circle", &a)
        }
        Part::Shape { d, transform, .. } => {
            let mut a = vec![
                ("d", d.clone()),
                ("transform", transform.clone()),
                ("fill", sv(t)),
            ];
            a.extend(n);
            r2("path", &a)
        }
        Part::Stroke { d, width, .. } => {
            let mut a = vec![
                ("d", d.clone()),
                ("fill", sv("none")),
                ("stroke", sv(t)),
                ("stroke-width", me(*width, 2)),
                ("stroke-linecap", sv("round")),
                ("stroke-linejoin", sv("round")),
            ];
            a.extend(n);
            r2("path", &a)
        }
    }
}

// ---------------------------------------------------------------------------
// `BE`/`HE`/`VE`/`bc` — the general/morph branch
// ---------------------------------------------------------------------------

/// `BE`'s record.
struct Be {
    size: f64,
    center: V2,
    body: Vec<V2>,
    eyes: Vec<Vec<V2>>,
    head: Option<We>,
    parts: Vec<Part>,
    mark_scale: f64,
    zoom: f64,
}

fn be(e: &Frame, t: f64, n: f64, tab: &Tables) -> Be {
    let mut r: Vec<(ShapeRec, f64)> = e
        .bodies
        .iter()
        .map(|b| (bc(e, b.body, t, tab), b.weight))
        .collect();
    r.sort_by(|l, d| d.1.partial_cmp(&l.1).unwrap_or(std::cmp::Ordering::Equal));
    let o = r.first().map(|(s, _)| s.center).unwrap_or([t / 2.0, t / 2.0]);
    let s = r.first().map(|(s, _)| s.radius).unwrap_or(0.0);
    let mut angles_src: Vec<Vec<f64>> = vec![om(192)];
    angles_src.extend(r.iter().map(|(l, _)| lm(&l.body, l.center)));
    let i = nm(angles_src);
    let a = lh(&r
        .iter()
        .map(|(l, d)| (fm(&l.body, l.center, &i), *d))
        .collect::<Vec<_>>());
    let c = e.morph.as_ref().map(|m| de(m, n, tab));
    let u = ON * t / 2.0 / X;
    Be {
        size: t,
        center: o,
        body: match &c {
            None => a.clone(),
            Some(c) if c.core.round == 0.0 => a.clone(),
            Some(c) => vm(&a, &ge2(c, o, s, &i), c.core.round),
        },
        eyes: if r.len() == 1 {
            r[0].0.eyes.clone()
        } else {
            ke(&r)
        },
        head: match (&c, &e.morph) {
            (Some(c), Some(m)) => Some(we2(c, m.amount, e.zoom, u)),
            _ => None,
        },
        parts: c.as_ref().map(|c| c.parts.clone()).unwrap_or_default(),
        mark_scale: u,
        zoom: c.as_ref().map(|c| c.zoom).unwrap_or(1.0),
    }
}

/// `VE` — the general/morph tree.
fn ve(e: &Be, t: &PaintOpts, n: &str, r: f64) -> MarkTree {
    let o = e.size;
    let s = e.center;
    let i = cc(t);
    let a = ga(&e.body);
    let c: String = e.eyes.iter().map(|e| ga(e)).collect::<Vec<_>>().join("");
    let mut u: Vec<MarkTree> = Vec::new();
    let mut l = vec![r2("path", &[("d", a.clone()), ("fill", i.clone())])];
    if !c.is_empty() && t.cutout {
        u.push(re(
            "mask",
            &[
                ("id", format!("{}-holes", n)),
                ("maskUnits", sv("userSpaceOnUse")),
                ("x", sv("0")),
                ("y", sv("0")),
                ("width", un(o)),
                ("height", un(o)),
            ],
            vec![
                r2(
                    "rect",
                    &[("width", un(o)), ("height", un(o)), ("fill", sv("#fff"))],
                ),
                r2("path", &[("d", c.clone()), ("fill", sv("#000"))]),
            ],
        ));
        l[0] = r2(
            "path",
            &[
                ("d", a.clone()),
                ("fill", i.clone()),
                ("mask", format!("url(#{}-holes)", n)),
            ],
        );
    } else if !c.is_empty() {
        u.push(re(
            "clipPath",
            &[("id", format!("{}-body", n))],
            vec![r2("path", &[("d", a.clone())])],
        ));
        l.push(r2(
            "path",
            &[
                ("d", c.clone()),
                ("fill", uc(t)),
                ("clip-path", format!("url(#{}-body)", n)),
            ],
        ));
    }
    let f: Vec<MarkTree> = match &e.head {
        None => l,
        Some(d) => {
            let mut attrs = vec![(
                "transform",
                format!(
                    "translate({} {}) rotate({}) scale({}) translate({} {})",
                    me(s[0] + d.translate[0], 2),
                    me(s[1] + d.translate[1], 2),
                    me(d.rotation, 2),
                    me(d.scale, 5),
                    me(-s[0], 2),
                    me(-s[1], 2)
                ),
            )];
            if d.opacity < 1.0 {
                attrs.push(("opacity", me(d.opacity, 3)));
            }
            vec![re("g", &attrs, l)]
        }
    };
    let h: Vec<MarkTree> = if e.parts.is_empty() {
        vec![]
    } else {
        vec![re(
            "g",
            &[(
                "transform",
                format!(
                    "translate({} {}) scale({}) translate({} {})",
                    o / 2.0,
                    o / 2.0,
                    me(e.mark_scale, 5),
                    -X,
                    -X
                ),
            )],
            e.parts.iter().map(|m| ue(m, &i)).collect(),
        )]
    };
    let p = o / 2.0 + (YT2 - X) * e.mark_scale;
    let g: Vec<MarkTree> = if e.zoom == 1.0 {
        h.into_iter().chain(f).collect()
    } else {
        vec![re(
            "g",
            &[(
                "transform",
                format!(
                    "translate({} {}) scale({}) translate({} {})",
                    me(p, 2),
                    me(p, 2),
                    me(e.zoom, 5),
                    me(-p, 2),
                    me(-p, 2)
                ),
            )],
            h.into_iter().chain(f).collect(),
        )]
    };
    let mut kids: Vec<MarkTree> = Vec::new();
    if !u.is_empty() {
        kids.push(re("defs", &[], u));
    }
    kids.extend(g);
    kc(o, n, r, kids)
}

/// `HE` — the general/morph branch of `Uh`.
fn he(e: &Frame, t: &PaintOpts, n: f64, r: &str, o: f64, s: f64, tab: &Tables) -> MarkTree {
    ve(&be(e, n, o, tab), t, r, s)
}

// ---------------------------------------------------------------------------
// `Uh`
// ---------------------------------------------------------------------------

/// `Uh(e, t, n, r, o)` — frame → `MarkTree`.
pub(crate) fn uh(
    e: &Frame,
    t: &PaintOpts,
    n: f64,
    r: &str,
    o: &UhSize,
    tab: &Tables,
) -> MarkTree {
    let (first, rest) = match e.bodies.split_first() {
        Some((s, i)) => (Some(s), !i.is_empty()),
        None => (None, false),
    };
    if let Some(s) = first.filter(|_| !rest && e.morph.is_none()) {
        if s.body == "sphere" {
            ht(&jt(e, t, n, tab), t, r, o.extent)
        } else {
            am(&em(e, s.body, t, n), r, o.extent)
        }
    } else {
        he(
            e,
            t,
            n,
            r,
            o.display_size.unwrap_or(n),
            o.extent,
            tab,
        )
    }
}

#[allow(dead_code)]
fn _unused() {
    let _ = (Rc::new(()),);
}
