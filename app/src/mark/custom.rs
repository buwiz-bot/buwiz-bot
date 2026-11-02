//! Custom bodies: a rounded, puffed 3D body built at runtime from a 2D
//! outline, made the way the kit's lofted bodies (cloud, heartChubby) are.
//!
//! The kit's silhouettes were solved offline into the atlas (the solver
//! was not ported, see `bodies`). A custom body is the kit loft of its
//! outline (`sS`: scaled copies of the outline stacked on a squashed
//! sphere profile) and solves its silhouettes here, one atlas node at a
//! time on first use, into an [`Atlas`] that `atlas_outline` reads exactly
//! like a shipped one. Its eyes ride the loft's dome (`hl`'s `loft` seat)
//! and its resting frames are baked from the live engine at the kit's rest
//! pose in the format `paint_rest` reads. Every lookup keyed on a body name
//! (`cs`, `av`, `cv`, `atlas_of`, `zr_outline`, `rest_bytes`) asks [`find`]
//! first for a `custom:` name.
//!
//! A rich body ([`rich_body`], `rich:` names) is a custom body's geometry
//! with its own paint: a shell colour, decal layers lifted onto the loft's
//! front (`bodies::decal_loops`) and filled eyes. `render` draws it
//! (`rich_atlas`, and the morph branch's rich case) at the exact pose, so
//! it rests live in the kit rest pose instead of from baked frames.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::atlas_data::{ATLAS_K, ATLAS_STEP};
use super::bodies::{self, Atlas, AvEye, Outline, BV, CS, US, X};
use super::rest_data::{REST_EN, REST_LAYERS, REST_N};
use super::{render, shared_tables, Mark, MN, V2, V3};

const TAU: f64 = std::f64::consts::TAU;

/// The loft's half-depth (body units) for an outline as round as the
/// kit cloud, whose loft is 0.72 deep; thinner outlines get proportionally
/// thinner bodies (see [`loft_depth`]).
const CLOUD_DEPTH: f64 = 0.72;
/// Re-centerings of a node silhouette on its own area centroid (one is
/// enough: the house, the kit's one off-center body, matches its shipped
/// centers within half a pixel after one and gains nothing after more).
const CENTER_PASSES: usize = 1;
/// Rays per node silhouette.
const RAYS: usize = ATLAS_K;
/// Atlas columns (yaw -180..170) and rows (pitch -90..90) of a custom body.
const COLS: usize = (360.0 / ATLAS_STEP) as usize;
const ROWS: usize = (180.0 / ATLAS_STEP) as usize + 1;
/// The kit square's body size (`hn(1.0413, ..)`): a custom body's area
/// is matched to it.
const SQUARE_SIZE: f64 = 1.0413;
/// The largest custom body size (the kit tops out at 1.521, sparkle).
const MAX_SIZE: f64 = 1.5;
/// The name prefix of every custom body.
const PREFIX: &str = "custom:";
/// The name prefix of every rich body.
const RICH_PREFIX: &str = "rich:";
/// The most decal layers a rich body takes.
const MAX_LAYERS: usize = 8;

// ---------------------------------------------------------------------------
// the extrusion and its silhouettes
// ---------------------------------------------------------------------------

/// A lofted solid: rings of the outline (body units, y up) stacked back to
/// front, all with the same vertex count so neighbours connect vertex to
/// vertex.
pub(super) struct Solid {
    rings: Vec<Vec<V3>>,
}

impl Solid {
    /// The kit loft (`sS`) of `outline` (`Jt` points, z = 0) to
    /// `±depth`: 17 rings, each the outline scaled about its bounding-box
    /// centre by `cos(a)^0.72` at height `depth * sin(a)`.
    pub(super) fn loft(outline: &[V3], depth: f64) -> Solid {
        Solid {
            rings: bodies::ss(outline, depth).rings,
        }
    }

    /// The silhouette seen from atlas node (`yaw`, `pitch`) in degrees, in
    /// the node's view frame (x right, y up, body units): its center (the
    /// area centroid) and `RAYS` polar radii from it, ray `i` at
    /// angle `TAU * i / RAYS`. Where the outline folds back on a ray the
    /// radius is the farthest crossing (the outer envelope).
    pub(super) fn silhouette(&self, yaw: f64, pitch: f64) -> (V2, Vec<f64>) {
        let r = bodies::rows(yaw * MN, pitch * MN);
        let projected: Vec<Vec<V2>> = self
            .rings
            .iter()
            .map(|ring| {
                ring.iter()
                    .map(|p| {
                        [
                            p[0] * r[0] + p[1] * r[1] + p[2] * r[2],
                            p[0] * r[3] + p[1] * r[4] + p[2] * r[5],
                        ]
                    })
                    .collect()
            })
            .collect();
        let (mut x0, mut x1, mut y0, mut y1) = (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        );
        for p in projected.iter().flatten() {
            x0 = x0.min(p[0]);
            x1 = x1.max(p[0]);
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1]);
        }
        let mut center = [(x0 + x1) / 2.0, (y0 + y1) / 2.0];
        let mut radii = envelope(&projected, center);
        for _ in 0..CENTER_PASSES {
            center = star_centroid(center, &radii);
            radii = envelope(&projected, center);
        }
        (center, radii)
    }
}

/// The area centroid of the star polygon `center + radii[i] * u_i`.
fn star_centroid(center: V2, radii: &[f64]) -> V2 {
    let n = radii.len();
    let pt = |i: usize| {
        let t = TAU * (i % n) as f64 / n as f64;
        [center[0] + radii[i % n] * t.cos(), center[1] + radii[i % n] * t.sin()]
    };
    let (mut a, mut cx, mut cy) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (p, q) = (pt(i), pt(i + 1));
        let w = p[0] * q[1] - q[0] * p[1];
        a += w;
        cx += (p[0] + q[0]) * w;
        cy += (p[1] + q[1]) * w;
    }
    if a.abs() < 1e-12 {
        return center;
    }
    [cx / (3.0 * a), cy / (3.0 * a)]
}

/// Polar radii of the union of the projected slices and the bands between
/// them, from `center`.
fn envelope(projected: &[Vec<V2>], center: V2) -> Vec<f64> {
    let rays: Vec<V2> = (0..RAYS)
        .map(|i| {
            let t = TAU * i as f64 / RAYS as f64;
            [t.cos(), t.sin()]
        })
        .collect();
    let mut radii = vec![0.0f64; RAYS];
    for (i, poly) in projected.iter().enumerate() {
        let n = poly.len();
        for j in 0..n {
            reach(center, poly[j], poly[(j + 1) % n], &rays, &mut radii);
        }
        if let Some(next) = projected.get(i + 1) {
            for (a, b) in poly.iter().zip(next) {
                reach(center, *a, *b, &rays, &mut radii);
            }
        }
    }
    radii
}

/// Raise `radii` to the farthest point of segment `a`-`b` along each ray
/// from `center` that crosses it (only the rays inside its angular span
/// are visited; `rays` holds each ray's unit vector).
fn reach(center: V2, a: V2, b: V2, rays: &[V2], radii: &mut [f64]) {
    let (ax, ay) = (a[0] - center[0], a[1] - center[1]);
    let (bx, by) = (b[0] - center[0], b[1] - center[1]);
    let (dx, dy) = (bx - ax, by - ay);
    let start = ay.atan2(ax);
    let mut span = by.atan2(bx) - start;
    if span > std::f64::consts::PI {
        span -= TAU;
    } else if span < -std::f64::consts::PI {
        span += TAU;
    }
    let (lo, hi) = if span >= 0.0 {
        (start, start + span)
    } else {
        (start + span, start)
    };
    let step = TAU / radii.len() as f64;
    let n = radii.len() as i64;
    for k in (lo / step).ceil() as i64..=(hi / step).floor() as i64 {
        let slot = k.rem_euclid(n) as usize;
        let [ux, uy] = rays[slot];
        let den = ux * dy - uy * dx;
        let s = if den.abs() < 1e-12 {
            (ax * ux + ay * uy).max(bx * ux + by * uy)
        } else {
            (ax * dy - ay * dx) / den
        };
        if s > radii[slot] {
            radii[slot] = s;
        }
    }
}

/// Signed area (positive counter-clockwise in y-up coordinates).
fn signed_area(poly: &[V2]) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (p, q) = (poly[i], poly[(i + 1) % n]);
            p[0] * q[1] - q[0] * p[1]
        })
        .sum::<f64>()
        / 2.0
}

/// A mark-box outline path as a polygon in body units (y up), resampled so
/// neighbouring vertices sit at least 2 mark px apart.
pub(super) fn body_polygon(path: &str) -> Vec<V2> {
    let mut out: Vec<V2> = Vec::new();
    for p in bodies::us(path, 4.0) {
        if out
            .last()
            .is_none_or(|q| (p[0] - q[0]).hypot(p[1] - q[1]) >= 2.0)
        {
            out.push(p);
        }
    }
    while out.len() > 3 {
        let (f, l) = (out[0], out[out.len() - 1]);
        if (f[0] - l[0]).hypot(f[1] - l[1]) < 2.0 {
            out.pop();
        } else {
            break;
        }
    }
    out.iter()
        .map(|&[x, y]| [(x - X) / X, -(y - X) / X])
        .collect()
}

/// A custom body's atlas nodes, each solved on first read and kept.
pub(crate) struct BakedNodes {
    solid: Solid,
    cache: RefCell<Vec<Option<Rc<[f64]>>>>,
}

impl BakedNodes {
    fn new(solid: Solid) -> BakedNodes {
        BakedNodes {
            solid,
            cache: RefCell::new(vec![None; COLS * ROWS]),
        }
    }

    /// Node `index` (`row * COLS + col`) in the atlas's byte units for an
    /// atlas with `c = 1` and `m = 255`: center bytes, then radii.
    pub(super) fn node(&self, index: usize) -> Rc<[f64]> {
        if let Some(n) = &self.cache.borrow()[index] {
            return n.clone();
        }
        let yaw = -180.0 + (index % COLS) as f64 * ATLAS_STEP;
        let pitch = -90.0 + (index / COLS) as f64 * ATLAS_STEP;
        let (center, radii) = self.solid.silhouette(yaw, pitch);
        let node: Rc<[f64]> = [(center[0] + 1.0) * 127.5, (center[1] + 1.0) * 127.5]
            .into_iter()
            .chain(radii)
            .collect();
        self.cache.borrow_mut()[index] = Some(node.clone());
        node
    }
}

// ---------------------------------------------------------------------------
// path input
// ---------------------------------------------------------------------------

enum Token {
    Command(char),
    Number(f64),
}

fn tokenize(path: &str) -> Result<Vec<Token>, String> {
    let b = path.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_whitespace() || c == ',' {
            i += 1;
        } else if matches!(c, 'M' | 'L' | 'C' | 'Q' | 'Z' | 'm' | 'l' | 'c' | 'q' | 'z') {
            out.push(Token::Command(c));
            i += 1;
        } else if c == '-' || c == '+' || c == '.' || c.is_ascii_digit() {
            let mut j = i + 1;
            let mut dot = c == '.';
            while j < b.len() {
                let d = b[j] as char;
                if d.is_ascii_digit() {
                    j += 1;
                } else if d == '.' && !dot {
                    dot = true;
                    j += 1;
                } else if (d == 'e' || d == 'E')
                    && b.get(j + 1).is_some_and(|n| {
                        n.is_ascii_digit()
                            || ((*n == b'-' || *n == b'+')
                                && b.get(j + 2).is_some_and(u8::is_ascii_digit))
                    })
                {
                    j += 2;
                    while j < b.len() && b[j].is_ascii_digit() {
                        j += 1;
                    }
                    break;
                } else {
                    break;
                }
            }
            let v: f64 = path[i..j]
                .parse()
                .map_err(|_| format!("bad number \"{}\"", &path[i..j]))?;
            if !v.is_finite() {
                return Err(format!("bad number \"{}\"", &path[i..j]));
            }
            out.push(Token::Number(v));
            i = j;
        } else if c.is_ascii_alphabetic() {
            return Err(format!("unsupported path command '{c}' (use M, L, C, Q, Z)"));
        } else {
            return Err(format!("unexpected character '{c}' in the path"));
        }
    }
    Ok(out)
}

/// Split SVG path data (M/L/C/Q/Z, absolute or relative) into absolute
/// subpaths, each a path string `us` reads.
fn subpaths(path: &str) -> Result<Vec<String>, String> {
    let tokens = tokenize(path)?;
    let mut subs: Vec<String> = Vec::new();
    let mut cur_sub = String::new();
    let mut cur = [0.0f64; 2];
    let mut start = [0.0f64; 2];
    let mut cmd: Option<char> = None;
    let mut i = 0;
    let fmt = |v: f64| format!("{}", (v * 1e4).round() / 1e4);
    while i < tokens.len() {
        let c = match tokens[i] {
            Token::Command(c) => {
                i += 1;
                c
            }
            Token::Number(_) => match cmd {
                // a repeated moveto is a lineto
                Some('M') => 'L',
                Some('m') => 'l',
                Some(c) if !matches!(c, 'Z' | 'z') => c,
                _ => return Err("path data must start with a command".to_owned()),
            },
        };
        cmd = Some(c);
        let upper = c.to_ascii_uppercase();
        if upper == 'Z' {
            if !cur_sub.is_empty() {
                cur_sub += "Z";
                subs.push(std::mem::take(&mut cur_sub));
            }
            cur = start;
            continue;
        }
        let n = match upper {
            'M' | 'L' => 2,
            'Q' => 4,
            _ => 6,
        };
        let mut args = Vec::with_capacity(n);
        for _ in 0..n {
            match tokens.get(i) {
                Some(Token::Number(v)) => args.push(*v),
                _ => return Err(format!("'{c}' needs {n} numbers")),
            }
            i += 1;
        }
        if c.is_ascii_lowercase() {
            for (k, v) in args.iter_mut().enumerate() {
                *v += cur[k % 2];
            }
        }
        let end = [args[n - 2], args[n - 1]];
        if upper == 'M' {
            if !cur_sub.is_empty() {
                subs.push(std::mem::take(&mut cur_sub));
            }
            start = end;
            cur_sub = format!("M{} {}", fmt(end[0]), fmt(end[1]));
        } else {
            if cur_sub.is_empty() {
                cur_sub = format!("M{} {}", fmt(cur[0]), fmt(cur[1]));
                start = cur;
            }
            cur_sub.push(upper);
            let nums: Vec<String> = args.iter().map(|&v| fmt(v)).collect();
            cur_sub += &nums.join(" ");
        }
        cur = end;
    }
    if !cur_sub.is_empty() {
        subs.push(cur_sub);
    }
    Ok(subs)
}

// ---------------------------------------------------------------------------
// eye placement
// ---------------------------------------------------------------------------

/// Distance from `p` to the polygon's boundary, negative outside.
fn signed_distance(poly: &[V2], p: V2) -> f64 {
    let n = poly.len();
    let mut best = f64::INFINITY;
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < a[0] + (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1])
        {
            inside = !inside;
        }
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l2 = dx * dx + dy * dy;
        let t = if l2 > 0.0 {
            (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        best = best.min((p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy));
    }
    if inside {
        best
    } else {
        -best
    }
}

/// The largest circle inside the polygon: center and radius (a grid
/// search refined three times around the best cell).
pub(super) fn inscribed_circle(poly: &[V2]) -> (V2, f64) {
    let (mut x0, mut x1, mut y0, mut y1) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for p in poly {
        x0 = x0.min(p[0]);
        x1 = x1.max(p[0]);
        y0 = y0.min(p[1]);
        y1 = y1.max(p[1]);
    }
    let mut center = [(x0 + x1) / 2.0, (y0 + y1) / 2.0];
    let mut best = signed_distance(poly, center);
    let (mut hw, mut hh) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
    const GRID: usize = 24;
    for _ in 0..4 {
        let c = center;
        for i in 0..=GRID {
            for j in 0..=GRID {
                let p = [
                    c[0] - hw + 2.0 * hw * i as f64 / GRID as f64,
                    c[1] - hh + 2.0 * hh * j as f64 / GRID as f64,
                ];
                let d = signed_distance(poly, p);
                if d > best {
                    best = d;
                    center = p;
                }
            }
        }
        hw *= 4.0 / GRID as f64;
        hh *= 4.0 / GRID as f64;
    }
    (center, best.max(0.0))
}

/// The kit square's eye box (`av("square")`) at its body size
/// [`SQUARE_SIZE`]: a custom body's eyes are scaled to render at the same
/// pixel size and spacing at the same display size.
const SQUARE_EYE: [f64; 3] = [0.2553258426966292, 0.559752808988764, 0.529408];
/// How far above the inscribed circle's center the eyes sit, per unit of
/// its radius: in the upper part of the face, not its middle.
const EYE_RISE_PER_R: f64 = 0.15;
/// Room kept between an eye and the outline (body units).
const EYE_MARGIN: f64 = 0.06;
/// The eyes stay inside the loft ring this far out from its center (the
/// kit cloud's sit inside about 0.6): past it the dome falls away fast
/// and an eye laid on its tangent plane would stick out of the side.
const EYE_REACH: f64 = 0.65;

/// Eyes for an outline of body `size` with largest inscribed circle
/// (`c`, `r`), and the x of their midpoint: the
/// kit square's eyes at the same rendered size, placed in the upper part
/// of the outline's largest inscribed circle, then shrunk (about their
/// center) only if an eye oval would cross the outline or reach the
/// steep rim of the loft (past [`EYE_REACH`] of the way out).
fn place_eyes(poly: &[V2], size: f64, (c, r): (V2, f64)) -> (AvEye, f64) {
    let (mut x0, mut x1, mut y0, mut y1) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for p in poly {
        x0 = x0.min(p[0]);
        x1 = x1.max(p[0]);
        y0 = y0.min(p[1]);
        y1 = y1.max(p[1]);
    }
    // the loft's center (`sS` scales its rings about the bounding box's)
    let mid = [(x0 + x1) / 2.0, (y0 + y1) / 2.0];
    let y = c[1] + EYE_RISE_PER_R * r;
    let base = SQUARE_SIZE / size;
    let mut k = 1.0;
    for _ in 0..40 {
        let [w, h, gap] = SQUARE_EYE.map(|v| v * base * k);
        // the oval at full open plus a widened/taller expression's slack
        let clear = [-1.0, 1.0].iter().all(|side| {
            (0..32).all(|i| {
                let t = TAU * i as f64 / 32.0;
                let p = [
                    c[0] + side * gap / 2.0 + t.cos() * w * 0.6,
                    y + t.sin() * h * 0.6,
                ];
                let reach = [
                    mid[0] + (p[0] - mid[0]) / EYE_REACH,
                    mid[1] + (p[1] - mid[1]) / EYE_REACH,
                ];
                signed_distance(poly, p) >= EYE_MARGIN * k && signed_distance(poly, reach) >= 0.0
            })
        });
        if clear {
            break;
        }
        k *= 0.95;
    }
    let [width, height, gap] = SQUARE_EYE.map(|v| v * base * k);
    let eyes = AvEye {
        width,
        height,
        gap,
        // the seats lower the eyes by `sin(shift_y)`
        shift_y: (-y).clamp(-0.9, 0.9).asin(),
    };
    (eyes, c[0])
}

/// The loft half-depth for an outline whose largest inscribed circle has
/// radius `r` (body units): the cloud's 0.72 for an outline as round as a
/// disc, thinner in proportion for narrow ones (a receipt), never below
/// 0.45 so it still reads as a rounded body from the side.
fn loft_depth(r: f64) -> f64 {
    (CLOUD_DEPTH * r).clamp(0.45, CLOUD_DEPTH)
}

// ---------------------------------------------------------------------------
// resting frames
// ---------------------------------------------------------------------------

/// A closed screen-space loop (y down) as the kit's resting frames store
/// one: clockwise on screen, starting where it crosses the vertical through
/// its area centroid on top, then `n` points evenly spaced by arc
/// length. One rule for every frame keeps point `i` on the same feature
/// from frame to frame, which `paint_rest`'s blending relies on.
fn resample(ring: &[V2], n: usize) -> Vec<V2> {
    let mut ring = ring.to_vec();
    let m = ring.len();
    if m >= 3 {
        let area: f64 = (0..m)
            .map(|i| {
                let (a, b) = (ring[i], ring[(i + 1) % m]);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum();
        if area < 0.0 {
            ring.reverse();
        }
        // the area centroid
        let (mut cx, mut cy) = (0.0, 0.0);
        for i in 0..m {
            let (a, b) = (ring[i], ring[(i + 1) % m]);
            let w = a[0] * b[1] - b[0] * a[1];
            cx += (a[0] + b[0]) * w;
            cy += (a[1] + b[1]) * w;
        }
        let area = area.abs().max(1e-9);
        let (cx, cy) = (cx / (3.0 * area), cy / (3.0 * area));
        let mut top: Option<(f64, usize, V2)> = None;
        for i in 0..m {
            let (a, b) = (ring[i], ring[(i + 1) % m]);
            if (a[0] - cx) * (b[0] - cx) > 0.0 || a[0] == b[0] {
                continue;
            }
            let t = (cx - a[0]) / (b[0] - a[0]);
            let y = a[1] + (b[1] - a[1]) * t;
            if y < cy && top.is_none_or(|(ty, _, _)| y < ty) {
                top = Some((y, i, [cx, y]));
            }
        }
        if let Some((_, i, p)) = top {
            let mut start = vec![p];
            start.extend(ring[i + 1..].iter().chain(&ring[..=i]));
            ring = start;
        }
    }
    let ring = &ring[..];
    let m = ring.len();
    if m == 0 {
        return vec![[0.0, 0.0]; n];
    }
    let seg = |i: usize| {
        let (a, b) = (ring[i], ring[(i + 1) % m]);
        (b[0] - a[0]).hypot(b[1] - a[1])
    };
    let total: f64 = (0..m).map(seg).sum();
    if total <= 0.0 {
        return vec![ring[0]; n];
    }
    let mut out = Vec::with_capacity(n);
    let (mut i, mut walked) = (0usize, 0.0f64);
    for k in 0..n {
        let target = total * k as f64 / n as f64;
        while i < m - 1 && walked + seg(i) < target {
            walked += seg(i);
            i += 1;
        }
        let (a, b) = (ring[i], ring[(i + 1) % m]);
        let t = if seg(i) > 0.0 {
            ((target - walked) / seg(i)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
    }
    out
}

/// One resting frame in `paint_rest`'s layout: the body ring (`REST_N`
/// points) then each eye (`REST_EN`), as x/y bytes of a 255px square.
fn rest_frame(mark: &Mark, body: &'static str, out: &mut Vec<u8>) {
    let (rings, eyes) = render::exact_rings(&mark.frame(), body, 255.0);
    let ring = rings.into_iter().max_by_key(|r| r.len()).unwrap_or_default();
    let mut loops = vec![resample(&ring, REST_N)];
    for i in 0..2 {
        loops.push(match eyes.get(i) {
            Some(e) => resample(e, REST_EN),
            // a closed-over eye: a point inside the body, so even-odd
            // cuts nothing
            None => vec![ring.first().copied().unwrap_or([0.0, 0.0]); REST_EN],
        });
    }
    for p in loops.iter().flatten() {
        out.push(p[0].round().clamp(0.0, 255.0) as u8);
        out.push(p[1].round().clamp(0.0, 255.0) as u8);
    }
}

/// Bake a body's resting frames the way the kit's were: the rest pose,
/// then for each hold layer of `REST_LAYERS` an n×n grid of gaze targets
/// (x across, y down the grid, both -1..1) held at that strength. A
/// resting mark following a pointer turns from its rest pose toward the
/// gaze pose by the hold (`gaze_step`'s not-playing branch), which is
/// what each frame is.
pub(super) fn bake_rest(body: &'static str) -> Rc<[u8]> {
    let tab = shared_tables();
    let mut mark = Mark::new(&tab, Some(body), 1.0, false, false, 1);
    let mut out = Vec::new();
    rest_frame(&mark, body, &mut out);
    for &(hold, n) in REST_LAYERS {
        for iy in 0..n {
            for ix in 0..n {
                let cx = -1.0 + 2.0 * ix as f64 / (n - 1) as f64;
                let cy = -1.0 + 2.0 * iy as f64 / (n - 1) as f64;
                mark.apply_pose();
                let g = super::fl(body, cx, cy);
                mark.u = [
                    mark.u[0] + (g.rx - mark.u[0]) * hold,
                    mark.u[1] + (g.ry - mark.u[1]) * hold,
                    mark.u[2] * (1.0 - hold),
                ];
                mark.l = [
                    mark.l[0] + (g.px - mark.l[0]) * hold,
                    mark.l[1] + (g.py - mark.l[1]) * hold,
                ];
                rest_frame(&mark, body, &mut out);
            }
        }
    }
    out.into()
}

/// The resting frames for a shape string naming a custom body: its
/// `custom:` name, or `outline:<path>` (the avatar's shape string, which
/// registers the body). `None` for anything else.
pub(crate) fn rest_bytes(shape: &str) -> Option<Rc<[u8]>> {
    let name = match shape.strip_prefix("outline:") {
        Some(path) => custom_body(path).ok()?,
        // a rich body rests live (its decals have no baked frames)
        None if shape.starts_with(PREFIX) => interned(shape)?,
        None => return None,
    };
    let body = find(name)?;
    Some(body.rest.get_or_init(|| bake_rest(name)).clone())
}

// ---------------------------------------------------------------------------
// the registry
// ---------------------------------------------------------------------------

/// A custom body: its outline record, seat size, loft, eye metrics,
/// atlas and resting frames.
pub(crate) struct CustomBody {
    pub outline: Rc<Outline>,
    /// How the input's 100×100 box maps onto the body's front.
    front: FrontMap,
    pub size: f64,
    /// The loft input (`Jt` points) and half-depth, for the `loft` seat.
    pub loft_points: Vec<V3>,
    pub depth: f64,
    pub eyes: AvEye,
    /// The eyes' horizontal center (body units); `cv` moves the loops there.
    pub eye_x: f64,
    pub atlas: Rc<Atlas>,
    /// The resting frames, baked on first use.
    rest: std::cell::OnceCell<Rc<[u8]>>,
}

/// The input box → body mapping of an outline: `Hv`'s centring and
/// scaling to `Bv`, then `hc`'s own renormalization step if it took one.
struct FrontMap {
    scale: f64,
    center: V2,
    /// `Vv`'s `(k, dx, dy)`, or identity.
    renorm: [f64; 3],
}

impl FrontMap {
    /// A point of the 100×100 input box (y down) in body units (y up).
    fn to_body(&self, p: V2) -> V2 {
        let [k, dx, dy] = self.renorm;
        let x = X + (X + (p[0] - self.center[0]) * self.scale + dx - X) * k;
        let y = X + (X + (p[1] - self.center[1]) * self.scale + dy - X) * k;
        [(x - X) / X, -(y - X) / X]
    }
}

thread_local! {
    static BY_PATH: RefCell<HashMap<String, &'static str>> = RefCell::new(HashMap::new());
    static BODIES: RefCell<HashMap<&'static str, Rc<CustomBody>>> = RefCell::new(HashMap::new());
}

/// The custom body registered under `name` (on this thread).
pub(super) fn find(name: &str) -> Option<Rc<CustomBody>> {
    if !name.starts_with(PREFIX) && !name.starts_with(RICH_PREFIX) {
        return None;
    }
    BODIES.with(|b| b.borrow().get(name).cloned())
}

/// The interned name of a registered custom or rich body.
pub(super) fn interned(name: &str) -> Option<&'static str> {
    if !name.starts_with(PREFIX) && !name.starts_with(RICH_PREFIX) {
        return None;
    }
    BODIES.with(|b| b.borrow().get_key_value(name).map(|(k, _)| *k))
}

fn fnv(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Polygon area in body units of the kit square's outline.
fn square_area() -> f64 {
    signed_area(&body_polygon(&bodies::zr_outline("square").path)).abs()
}

/// Build a body from a 100×100 (y down) outline.
fn build(path: &str) -> Result<CustomBody, String> {
    let subs = subpaths(path)?;
    let mut best: Option<(f64, Vec<V2>)> = None;
    for s in &subs {
        let pts = bodies::us(s, 0.5);
        if pts.len() < 3 {
            continue;
        }
        let a = signed_area(&pts).abs();
        if best.as_ref().is_none_or(|b| a > b.0) {
            best = Some((a, pts));
        }
    }
    let Some((area, pts)) = best else {
        return Err("the outline needs at least 3 points".to_owned());
    };
    // 1 unit² of the 100×100 box: a sliver, nothing to draw
    if area.is_nan() || area < 1.0 {
        return Err("the outline encloses no area".to_owned());
    }
    let (mut x0, mut x1, mut y0, mut y1) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for p in &pts {
        x0 = x0.min(p[0]);
        x1 = x1.max(p[0]);
        y0 = y0.min(p[1]);
        y1 = y1.max(p[1]);
    }
    // the kit's `Hv` normalization: the longer side spans `Bv`, centred
    let scale = BV / (x1 - x0).max(y1 - y0);
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let mut d = String::new();
    for (i, p) in pts.iter().enumerate() {
        let x = ((X + (p[0] - cx) * scale) * 100.0).round() / 100.0;
        let y = ((X + (p[1] - cy) * scale) * 100.0).round() / 100.0;
        d += &format!("{}{x} {y}", if i == 0 { "M" } else { "L" });
    }
    d += "Z";
    // `hc` renormalizes (`Vv`) unless the path is already within its
    // tolerance; keep the same step for anything drawn on this front
    let (k, dx, dy) = bodies::hv(&bodies::us(&d, 4.0));
    let renorm = (k - 1.0).abs() >= 0.005 || dx.abs() >= 0.5 || dy.abs() >= 0.5;
    let front = FrontMap {
        scale,
        center: [cx, cy],
        renorm: if renorm { [k, dx, dy] } else { [1.0, 0.0, 0.0] },
    };
    let outline = Rc::new(bodies::hc(&d));
    let poly = body_polygon(&outline.path);
    let poly_area = signed_area(&poly).abs();
    if poly.len() < 3 || poly_area < 1e-3 {
        return Err("the outline encloses no area".to_owned());
    }
    let size = (SQUARE_SIZE * (square_area() / poly_area).sqrt()).clamp(SQUARE_SIZE, MAX_SIZE);
    let circle = inscribed_circle(&poly);
    let (eyes, eye_x) = place_eyes(&poly, size, circle);
    let depth = loft_depth(circle.1);
    let loft_points = bodies::jt(&bodies::us(&outline.path, 1.0));
    let atlas = Rc::new(Atlas {
        mirror: false,
        m: 255.0,
        c: 1.0,
        kf: size * CS / US,
        step: ATLAS_STEP,
        k: ATLAS_K,
        bytes: Vec::new(),
        baked: Some(BakedNodes::new(Solid::loft(&loft_points, depth))),
    });
    Ok(CustomBody {
        outline,
        front,
        size,
        loft_points,
        depth,
        eyes,
        eye_x,
        atlas,
        rest: std::cell::OnceCell::new(),
    })
}

/// Register a body drawn from a new 2D outline and return its name, for
/// `Mark::new`/`Mark::set_body` like a kit body's.
///
/// `path` is SVG path data (M/L/C/Q/Z, absolute or relative) in a
/// `0 0 100 100` box, y down. The body is a lofted, rounded body like the
/// kit's cloud: centred and scaled to the kit square's visual area, its
/// silhouettes solved on first view of each angle, its eyes the kit
/// square's size on its curved front, its resting frames baked on first
/// rest. With several subpaths the largest one is the body.
///
/// The same `path` always returns the same name (bodies live for the
/// thread, one leaked name per distinct outline). Errors are short
/// human-readable messages: unparseable data, fewer than 3 points, or no
/// enclosed area.
pub(crate) fn custom_body(path: &str) -> Result<&'static str, String> {
    let key = path.trim();
    if let Some(name) = BY_PATH.with(|m| m.borrow().get(key).copied()) {
        return Ok(name);
    }
    let body = Rc::new(build(key)?);
    let hash = fnv(key);
    let mut name = format!("{PREFIX}{hash:016x}");
    let mut n = 1;
    while BODIES.with(|b| b.borrow().contains_key(name.as_str())) {
        name = format!("{PREFIX}{hash:016x}-{n}");
        n += 1;
    }
    let name: &'static str = Box::leak(name.into_boxed_str());
    BODIES.with(|b| b.borrow_mut().insert(name, body));
    BY_PATH.with(|m| m.borrow_mut().insert(key.to_owned(), name));
    Ok(name)
}

// ---------------------------------------------------------------------------
// rich bodies: a custom body with its own colours and surface decals
// ---------------------------------------------------------------------------

/// One decal layer of a rich body: closed loops on the body's front (body
/// units, y up, densely sampled so they bend with the surface), a fill and
/// an opacity.
pub(crate) struct Decal {
    pub loops: Vec<Vec<V2>>,
    pub fill: String,
    pub opacity: f64,
}

/// A rich body's paint: the shell, the decals back to front, the eyes.
pub(crate) struct Rich {
    pub shell: String,
    pub eyes: String,
    pub decals: Vec<Decal>,
}

thread_local! {
    static BY_RECIPE: RefCell<HashMap<String, &'static str>> = RefCell::new(HashMap::new());
    static RICH: RefCell<HashMap<&'static str, Rc<Rich>>> = RefCell::new(HashMap::new());
}

/// The paint of the rich body registered under `name` (on this thread).
pub(super) fn rich(name: &str) -> Option<Rc<Rich>> {
    if !name.starts_with(RICH_PREFIX) {
        return None;
    }
    RICH.with(|r| r.borrow().get(name).cloned())
}

/// A `#rgb`/`#rrggbb` colour, uppercased, or an error naming `what`.
fn colour(v: Option<&serde_json::Value>, what: &str) -> Result<String, String> {
    let s = v
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("the recipe needs a {what} colour"))?
        .trim();
    let hex = s.strip_prefix('#').unwrap_or("");
    if !matches!(hex.len(), 3 | 6) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("bad {what} colour \"{s}\" (use #rrggbb)"));
    }
    let hex = if hex.len() == 3 {
        hex.chars().flat_map(|c| [c, c]).collect::<String>()
    } else {
        hex.to_owned()
    };
    Ok(format!("#{}", hex.to_uppercase()))
}

/// Points of a loop at least `step` apart along it, closing gaps longer
/// than `step` with evenly spaced points.
fn densify(points: &[V2], step: f64) -> Vec<V2> {
    let n = points.len();
    let mut out = Vec::new();
    for i in 0..n {
        let (a, b) = (points[i], points[(i + 1) % n]);
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        let k = (len / step).ceil().max(1.0) as usize;
        for j in 0..k {
            let t = j as f64 / k as f64;
            out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
        }
    }
    out
}

/// Register a multi-colour body from a recipe and return its name, for
/// `Mark::new`/`Mark::set_body` like a kit body's.
///
/// The recipe is compact JSON:
/// `{"o":"<outline>","c":"#shell","l":[["<path>","#fill",opacity],…],"e":"#eyes"}`.
/// `o` builds (or reuses) the plain [`custom_body`] for that outline, so
/// the geometry, atlas and eye seat are shared with it. `l` holds up to 8
/// decal layers, back to front: closed M/L/C/Q/Z paths in the same
/// 0..100 front-view box as `o` (y down), each subpath a loop, the opacity
/// optional (default 1). The decals ride the body's curved front; the
/// shell, decals and eyes take the recipe's colours instead of the ink.
///
/// The same recipe always returns the same name (`rich:<hash>`). Errors
/// are short human-readable messages: bad JSON, a missing or bad colour,
/// an unusable outline or layer path, more than 8 layers.
pub(crate) fn rich_body(json: &str) -> Result<&'static str, String> {
    let key = json.trim();
    if let Some(name) = BY_RECIPE.with(|m| m.borrow().get(key).copied()) {
        return Ok(name);
    }
    let v: serde_json::Value =
        serde_json::from_str(key).map_err(|e| format!("the recipe isn't JSON: {e}"))?;
    let outline = v
        .get("o")
        .and_then(|o| o.as_str())
        .ok_or("the recipe needs an outline (\"o\")")?;
    let base_name = custom_body(outline)?;
    let base = find(base_name).ok_or("the outline didn't register")?;
    let shell = colour(v.get("c"), "shell")?;
    let eyes = colour(v.get("e"), "eye")?;
    let layers = match v.get("l") {
        None | Some(serde_json::Value::Null) => &[][..],
        Some(serde_json::Value::Array(a)) => &a[..],
        Some(_) => return Err("the layers (\"l\") must be a list".to_owned()),
    };
    if layers.len() > MAX_LAYERS {
        return Err(format!("at most {MAX_LAYERS} layers, got {}", layers.len()));
    }
    let mut decals = Vec::with_capacity(layers.len());
    for (i, layer) in layers.iter().enumerate() {
        let n = i + 1;
        let parts = layer
            .as_array()
            .ok_or_else(|| format!("layer {n} must be [path, colour, opacity]"))?;
        let path = parts
            .first()
            .and_then(|p| p.as_str())
            .ok_or_else(|| format!("layer {n} needs a path"))?;
        let fill = colour(parts.get(1), &format!("layer {n}"))?;
        let opacity = match parts.get(2) {
            None => 1.0,
            Some(o) => o
                .as_f64()
                .filter(|o| o.is_finite())
                .ok_or_else(|| format!("layer {n} opacity must be a number"))?
                .clamp(0.0, 1.0),
        };
        let mut loops = Vec::new();
        for sub in subpaths(path).map_err(|e| format!("layer {n}: {e}"))? {
            let pts = bodies::us(&sub, 0.5);
            let area = signed_area(&pts).abs();
            if pts.len() < 3 || area.is_nan() || area < 0.25 {
                continue;
            }
            let body: Vec<V2> = pts.iter().map(|&p| base.front.to_body(p)).collect();
            loops.push(densify(&body, 0.03));
        }
        if loops.is_empty() {
            return Err(format!("layer {n} encloses no area"));
        }
        decals.push(Decal {
            loops,
            fill,
            opacity,
        });
    }
    let name: &'static str =
        Box::leak(format!("{RICH_PREFIX}{:016x}", fnv(key)).into_boxed_str());
    BODIES.with(|b| b.borrow_mut().insert(name, base));
    RICH.with(|r| {
        r.borrow_mut().insert(
            name,
            Rc::new(Rich {
                shell,
                eyes,
                decals,
            }),
        )
    });
    BY_RECIPE.with(|m| m.borrow_mut().insert(key.to_owned(), name));
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::super::{
        shared_tables, uh, yn, Director, Mark, MarkTree, PaintOpts, UhSize, BOT_SHAPES,
    };
    use super::*;

    const CAT: &str = "M20 92 C8 80 8 55 14 40 L10 6 L38 26 C46 23 54 23 62 26 L90 6 \
                       L86 40 C92 55 92 80 80 92 C64 100 36 100 20 92 Z";
    /// A receipt with a zigzag tear (relative) and a folded corner (a
    /// second, smaller subpath).
    const RECEIPT: &str = "M22 6 L64 6 L78 20 L78 94 l-7 -5 l-7 5 l-7 -5 l-7 5 l-7 -5 \
                           l-7 5 l-7 -5 l-7 5 Z M64 6 L64 20 L78 20 Z";

    fn feed(h: &mut u64, s: &str) {
        for b in s.bytes().chain(std::iter::once(0xff)) {
            *h ^= b as u64;
            *h = h.wrapping_mul(0x100000001b3);
        }
    }

    /// FNV-1a over a tree's tags, attributes and children, in order.
    fn hash_tree(tree: &MarkTree, h: &mut u64) {
        feed(h, tree.tag);
        for (k, v) in &tree.attrs {
            feed(h, k);
            feed(h, v);
        }
        feed(h, "(");
        for c in &tree.children {
            hash_tree(c, h);
        }
        feed(h, ")");
    }

    fn render(mark: &Mark) -> MarkTree {
        let tab = shared_tables();
        uh(
            &mark.frame(),
            &PaintOpts {
                ink: [0.0, 0.0, 0.0],
                spot: [0.0, 0.0, 0.0],
                cutout: true,
                ink_paint: Some("#1884FC".to_owned()),
                spot_paint: None,
            },
            144.0,
            "m",
            &UhSize {
                display_size: Some(96.0),
                extent: 144.0,
            },
            &tab,
        )
    }

    fn step(dir: &mut Director, mark: &mut Mark) {
        dir.advance(mark, 1.0 / 30.0);
        mark.advance(1.0 / 30.0);
    }

    /// One hash per picker shape: frames of two states, then a morph to the
    /// next shape in the picker and back.
    fn kit_hashes() -> Vec<(&'static str, u64)> {
        let tab = shared_tables();
        BOT_SHAPES
            .iter()
            .enumerate()
            .map(|(i, shape)| {
                let body = yn(shape);
                let mut h = 0xcbf29ce484222325u64;
                let mut mark = Mark::new(&tab, Some(body), 1.0, false, false, 7);
                let mut dir = Director::new(&mut mark, "thinking", 7, false, false, None);
                for f in 0..60 {
                    step(&mut dir, &mut mark);
                    if f % 6 == 0 {
                        hash_tree(&render(&mark), &mut h);
                    }
                    if f == 30 {
                        dir.set_state_pub(&mut mark, "surprised");
                    }
                }
                let next = yn(BOT_SHAPES[(i + 1) % BOT_SHAPES.len()]);
                for target in [next, body] {
                    mark.set_body(target, false);
                    for f in 0..40 {
                        step(&mut dir, &mut mark);
                        if f % 4 == 0 {
                            hash_tree(&render(&mark), &mut h);
                        }
                    }
                }
                (*shape, h)
            })
            .collect()
    }

    /// The kit hash run for one body, morphing to `next` and back.
    fn body_hash(body: &'static str, next: &'static str) -> u64 {
        let tab = shared_tables();
        let mut h = 0xcbf29ce484222325u64;
        let mut mark = Mark::new(&tab, Some(body), 1.0, false, false, 7);
        let mut dir = Director::new(&mut mark, "thinking", 7, false, false, None);
        for f in 0..60 {
            step(&mut dir, &mut mark);
            if f % 6 == 0 {
                hash_tree(&render(&mark), &mut h);
            }
            if f == 30 {
                dir.set_state_pub(&mut mark, "surprised");
            }
        }
        for target in [next, body] {
            mark.set_body(target, false);
            for f in 0..40 {
                step(&mut dir, &mut mark);
                if f % 4 == 0 {
                    hash_tree(&render(&mark), &mut h);
                }
            }
        }
        h
    }

    /// Plain custom bodies (round 2) draw and rest byte for byte as they
    /// did before rich bodies existed.
    #[test]
    fn plain_custom_bodies_unchanged() {
        let mut got = Vec::new();
        for path in [CAT, RECEIPT] {
            let name = custom_body(path).unwrap();
            let mut h = body_hash(name, "hex");
            for b in super::super::paint::rest_bytes(name).unwrap().iter() {
                h ^= *b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            got.push(h);
        }
        let want: [u64; 2] = [0x9b46cb46d116b458, 0xd3a9bc4da368affd];
        assert_eq!(got, want.to_vec());
    }

    /// Captured before custom bodies existed: the kit draws byte for byte
    /// as it did.
    #[test]
    fn kit_bodies_unchanged() {
        let want: [(&str, u64); 13] = [
            ("blob", 0x3bc4e0873e350186),
            ("cloud", 0xac60b06ad1d947a9),
            ("square", 0x84513d2faeac8003),
            ("sparkle", 0x5c0bffee9d1905a3),
            ("clover", 0x45da81749f1d1f89),
            ("heart", 0x705ab54a5f3bd918),
            ("flower", 0xa5f4f32a828111fa),
            ("teardrop", 0xbdcb329ae6e751f2),
            ("tablet", 0x001bf662a04de000),
            ("wedge", 0x377d8faccce33da8),
            ("house", 0xf747922e65850c30),
            ("star", 0x01771f589437fc0c),
            ("hex", 0x2c8f8be6fb1c0112),
        ];
        // a custom body registered first must not disturb the kit
        custom_body(CAT).unwrap();
        assert_eq!(kit_hashes(), want.to_vec());
    }

    /// The runtime solver against the shipped atlas of the kit's lofted
    /// bodies: every node, every ray, in pixels of a 144px sprite.
    /// Returns (max, mean, mean radius, max center offset).
    fn compare(body: &'static str, solid: &Solid) -> (f64, f64, f64, f64) {
        let a = bodies::atlas_of(body).unwrap();
        let cols = if a.mirror { 19 } else { COLS };
        let px = a.kf * 144.0;
        let (mut max_e, mut sum_e, mut n, mut sum_r, mut max_c) = (0.0f64, 0.0, 0.0, 0.0, 0.0f64);
        for row in 0..ROWS {
            for col in 0..cols {
                let v = a.node(row * cols + col);
                let c = [(v[0] / 255.0 * 2.0 - 1.0) * a.c, (v[1] / 255.0 * 2.0 - 1.0) * a.c];
                let yaw = if a.mirror { col as f64 * 10.0 } else { -180.0 + col as f64 * 10.0 };
                let (mc, mr) = solid.silhouette(yaw, -90.0 + row as f64 * 10.0);
                max_c = max_c.max((mc[0] - c[0]).hypot(mc[1] - c[1]) * px);
                for (i, r) in mr.iter().enumerate() {
                    let shipped = v[2 + i] * a.m / 255.0;
                    let e = (r - shipped).abs() * px;
                    max_e = max_e.max(e);
                    sum_e += e;
                    sum_r += shipped * px;
                    n += 1.0;
                }
            }
        }
        (max_e, sum_e / n, sum_r / n, max_c)
    }

    fn kit_lofts() -> [(&'static str, Solid); 2] {
        let heart: Vec<V3> = super::super::clips::HEART_OUTLINE
            .iter()
            .map(|&[x, y]| [x, y, 0.0])
            .collect();
        [
            ("cloud", Solid::loft(&bodies::jt(&bodies::us(&bodies::zr_outline("cloud").path, 1.0)), 0.72)),
            ("heartChubby", Solid::loft(&heart, 0.62)),
        ]
    }

    /// The rule: the kit loft (`sS` rings, `Hl`'s depth), node center =
    /// the silhouette's area centroid. The shipped radii are bytes of
    /// `m/255` (about 0.2px here), so a mean under half a byte is as close
    /// as the data can tell.
    #[test]
    fn solver_matches_shipped_atlas() {
        for (body, solid) in kit_lofts() {
            let (max_e, mean, mean_r, max_c) = compare(body, &solid);
            println!(
                "{body}: max {max_e:.3}px mean {mean:.3}px ({:.2}% of {mean_r:.1}px) \
                 center max {max_c:.3}px",
                mean / mean_r * 100.0
            );
            assert!(mean / mean_r <= 0.01, "{body} mean {mean}");
            assert!(max_e <= 1.0, "{body} max {max_e}");
            assert!(max_c <= 0.5, "{body} center {max_c}");
        }
    }

    /// Point-by-point error of two resting-frame sets (mean, max), in
    /// units of the 255 square.
    fn rest_error(a: &[u8], b: &[u8]) -> (f64, f64) {
        assert_eq!(a.len(), b.len());
        let (mut sum, mut max, mut n) = (0.0, 0.0f64, 0.0);
        for (p, q) in a.chunks(2).zip(b.chunks(2)) {
            let e = (p[0] as f64 - q[0] as f64).hypot(p[1] as f64 - q[1] as f64);
            sum += e;
            max = max.max(e);
            n += 1.0;
        }
        (sum / n, max)
    }

    /// The baker custom bodies rest with reproduces the kit's shipped
    /// resting frames (rest pose, every gaze grid frame of every hold
    /// layer, body and eyes) for atlas bodies with front and loft seats.
    #[test]
    fn rest_baker_reproduces_kit_rest_frames() {
        for (shape, body) in [("square", "square"), ("house", "house"), ("cloud", "cloud"), ("heart", "heartChubby")] {
            let kit = super::super::paint::rest_bytes(shape).unwrap();
            let (mean, max) = rest_error(&bake_rest(body), &kit);
            println!("{shape}: mean {mean:.3} max {max:.2} (of 255)");
            assert!(mean <= 1.0, "{shape} mean {mean}");
            assert!(max <= 8.0, "{shape} max {max}");
        }
    }

    /// A custom body rests in the kit's pose: the same rest rotation, the
    /// same baked gaze grid, eyes up toward the gaze side like the kit's.
    #[test]
    fn custom_rest_pose_matches_kit() {
        let tab = shared_tables();
        let name = custom_body(CAT).unwrap();
        let kit = Mark::new(&tab, Some("square"), 1.0, false, false, 1).frame();
        let ours = Mark::new(&tab, Some(name), 1.0, false, false, 1).frame();
        assert_eq!(kit.quat, ours.quat);
        let by_outline = super::super::paint::rest_bytes(&format!("outline:{CAT}")).unwrap();
        let by_name = super::super::paint::rest_bytes(name).unwrap();
        assert!(Rc::ptr_eq(&by_outline, &by_name));
        assert_eq!(by_name.len(), super::super::paint::rest_bytes("square").unwrap().len());
        // frame 0: both eyes sit above the body's middle and to its right,
        // as the kit's do at rest
        let center = |r: std::ops::Range<usize>| {
            let n = r.len() as f64;
            let (x, y) = r.fold((0.0, 0.0), |(x, y), i| {
                (x + by_name[i * 2] as f64, y + by_name[i * 2 + 1] as f64)
            });
            [x / n, y / n]
        };
        let body = center(0..REST_N);
        for eye in [center(REST_N..REST_N + REST_EN), center(REST_N + REST_EN..REST_N + 2 * REST_EN)] {
            assert!(eye[1] < body[1], "eye {eye:?} body {body:?}");
        }
        let eyes_x = (center(REST_N..REST_N + REST_EN)[0] + center(REST_N + REST_EN..REST_N + 2 * REST_EN)[0]) / 2.0;
        assert!(eyes_x > body[0], "eyes {eyes_x} body {body:?}");
    }

    /// Bounding-box width and height of each eye of resting frame 0.
    fn rest_eye_sizes(bytes: &[u8]) -> Vec<[f64; 2]> {
        [REST_N, REST_N + REST_EN]
            .iter()
            .map(|&from| {
                let (mut x0, mut x1, mut y0, mut y1) = (999.0f64, -1.0f64, 999.0f64, -1.0f64);
                for i in from..from + REST_EN {
                    let (x, y) = (bytes[i * 2] as f64, bytes[i * 2 + 1] as f64);
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                }
                [x1 - x0, y1 - y0]
            })
            .collect()
    }

    /// A custom body's eyes render at the kit square's size: a squarish
    /// outline and the cat against the kit square, same display size.
    #[test]
    fn eye_size_matches_kit() {
        let kit = rest_eye_sizes(&super::super::paint::rest_bytes("square").unwrap());
        for path in ["M14 10 L86 10 Q90 10 90 14 L90 86 Q90 90 86 90 L14 90 Q10 90 10 86 L10 14 Q10 10 14 10 Z", CAT] {
            let name = custom_body(path).unwrap();
            let ours = rest_eye_sizes(&super::super::paint::rest_bytes(name).unwrap());
            for (k, o) in kit.iter().zip(&ours) {
                let (rw, rh) = (o[0] / k[0], o[1] / k[1]);
                println!("{name}: eye {o:?} kit {k:?} ratio {rw:.2} {rh:.2}");
                assert!((0.85..=1.15).contains(&rw) && (0.85..=1.15).contains(&rh));
            }
        }
    }

    /// Width of a silhouette (body units) from its center and radii.
    fn width(center: V2, radii: &[f64]) -> f64 {
        let xs = radii.iter().enumerate().map(|(i, r)| {
            center[0] + r * (TAU * i as f64 / radii.len() as f64).cos()
        });
        let (lo, hi) = xs.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| (lo.min(x), hi.max(x)));
        hi - lo
    }

    /// Turned 40° either way the body is clearly wider than its front
    /// outline squashed by cos 40° (a coin), and its outline changes
    /// shape as it turns, like the kit's lofted bodies.
    #[test]
    fn silhouette_shows_depth() {
        for path in [CAT, RECEIPT] {
            let name = custom_body(path).unwrap();
            let b = find(name).unwrap();
            let solid = Solid::loft(&b.loft_points, b.depth);
            let (c0, r0) = solid.silhouette(0.0, 0.0);
            let coin = width(c0, &r0) * 40f64.to_radians().cos();
            for yaw in [-40.0, 40.0] {
                let (c, r) = solid.silhouette(yaw, 0.0);
                let w = width(c, &r);
                println!("{name} yaw {yaw}: width {w:.3} coin {coin:.3} ({:.2}x)", w / coin);
                assert!(w >= coin * 1.15, "{name} yaw {yaw}: {w} vs coin {coin}");
            }
            // side on, the body is a rounded form, not a card edge
            let (c, r) = solid.silhouette(90.0, 0.0);
            let side = width(c, &r);
            println!("{name} side width {side:.3} depth {:.3}", b.depth);
            assert!(side >= 1.8 * b.depth);
        }
    }

    /// Debug-build timings (printed; release is several times faster):
    /// registering a body, its first node, every node, every node cached,
    /// baking its resting frames.
    #[test]
    fn bake_timing() {
        let t = std::time::Instant::now();
        let name = custom_body("M50 4 C78 4 96 22 96 50 C96 78 78 96 50 96 L50 70 L30 96 \
                                C14 90 4 72 4 50 C4 22 22 4 50 4 Z")
        .unwrap();
        let build = t.elapsed();
        let a = bodies::atlas_of(name).unwrap();
        let t = std::time::Instant::now();
        a.node(0);
        let one = t.elapsed();
        let t = std::time::Instant::now();
        for i in 0..COLS * ROWS {
            a.node(i);
        }
        let all = t.elapsed();
        let t = std::time::Instant::now();
        for i in 0..COLS * ROWS {
            a.node(i);
        }
        let cached = t.elapsed();
        let t = std::time::Instant::now();
        let rest = bake_rest(name);
        println!(
            "register {build:?}, one node {one:?}, all {} nodes {all:?}, cached {cached:?}, \
             {} resting frames {:?}",
            COLS * ROWS,
            rest.len() / ((REST_N + 2 * REST_EN) * 2),
            t.elapsed()
        );
    }

    #[test]
    fn custom_body_names() {
        let a = custom_body(CAT).unwrap();
        assert!(a.starts_with("custom:"));
        assert_eq!(custom_body(CAT).unwrap(), a);
        assert_eq!(custom_body(&format!("  {CAT}\n")).unwrap(), a);
        let b = custom_body(RECEIPT).unwrap();
        assert_ne!(a, b);
        assert_eq!(yn(a), a);
        assert_eq!(yn("custom:nope"), "sphere");
        for bad in [
            "",
            "hello",
            "M10 10",
            "M0 0 L10 10",
            "M10 10 L20 20 L30 30 Z",
            "M0 0 A 1 1 0 0 0 50 50 Z",
            "M0 0 L50",
            "10 10 L 50 50",
        ] {
            assert!(custom_body(bad).is_err(), "{bad:?} should be rejected");
        }
        // relative and absolute spell the same outline
        let abs = subpaths("M10 10 L90 10 Q90 90 50 90 Z").unwrap();
        let rel = subpaths("m10 10 l80 0 q0 80 -40 80 z").unwrap();
        assert_eq!(abs, rel);
    }

    /// The `d` of every `path` under the node matching `pick`.
    fn paths(tree: &MarkTree, pick: &dyn Fn(&MarkTree) -> bool, inside: bool, out: &mut Vec<String>) {
        let hit = inside || pick(tree);
        if hit
            && tree.tag == "path"
            && let Some(d) = tree.attr("d")
        {
            out.push(d.to_owned());
        }
        for c in &tree.children {
            paths(c, pick, hit, out);
        }
    }

    fn loops(d: &str) -> Vec<Vec<V2>> {
        d.split('Z')
            .map(|l| {
                let v: Vec<f64> = l
                    .replace(['M', 'L'], " ")
                    .split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                v.chunks(2).map(|c| [c[0], c[1]]).collect::<Vec<V2>>()
            })
            .filter(|l| l.len() >= 3)
            .collect()
    }

    fn render_at(mark: &Mark, extent: f64, display: f64) -> MarkTree {
        let tab = shared_tables();
        uh(
            &mark.frame(),
            &PaintOpts {
                cutout: true,
                ink_paint: Some("#1884FC".to_owned()),
                ..Default::default()
            },
            extent,
            "m",
            &UhSize {
                display_size: Some(display),
                extent,
            },
            &tab,
        )
    }

    /// The body outline and eye loops of an atlas-branch tree.
    fn body_and_eyes(tree: &MarkTree) -> (Vec<Vec<V2>>, Vec<Vec<V2>>) {
        let mut body_d = Vec::new();
        paths(tree, &|t| t.attr("id") == Some("m-body"), false, &mut body_d);
        let mut eye_d = Vec::new();
        paths(tree, &|t| t.attr("id") == Some("m-holes"), false, &mut eye_d);
        (
            body_d.iter().flat_map(|d| loops(d)).collect(),
            eye_d.iter().flat_map(|d| loops(d)).collect(),
        )
    }

    /// The avatar's sizes: (extent, display size) of the 64/80/96pt
    /// avatars and a 40pt grid cell.
    const SIZES: [(f64, f64); 4] = [(80.47, 52.48), (100.59, 65.6), (120.71, 78.72), (50.29, 32.8)];

    /// One run of what an avatar does to its mark, frame by frame: resting
    /// with the pointer swept over a grid of gaze targets, then each
    /// `STATES` clip, then each poke. Returns, per frame, the eye loop
    /// count on the atlas branch (`None` for a morph frame).
    fn eye_counts(body: &'static str, extent: f64, display: f64) -> Vec<(String, Option<usize>)> {
        let tab = shared_tables();
        let mut out = Vec::new();
        let mut mark = Mark::new(&tab, Some(body), 1.0, false, false, 9);
        let mut dir = Director::new(&mut mark, "idle", 9, display < 48.0, true, None);
        let record = |mark: &Mark, label: String, out: &mut Vec<(String, Option<usize>)>| {
            let frame = mark.frame();
            let count = if frame.bodies.len() == 1 && frame.morph.is_none() {
                let (bodies, eyes) = body_and_eyes(&render_at(mark, extent, display));
                assert_eq!(bodies.len(), 1, "{label}: no body");
                Some(eyes.len())
            } else {
                None
            };
            out.push((label, count));
        };
        for gy in -2..=2 {
            for gx in -2..=2 {
                mark.set_gaze(Some([gx as f64 / 2.0, gy as f64 / 2.0]));
                for f in 0..12 {
                    mark.advance(1.0 / 30.0);
                    record(&mark, format!("rest gaze {gx},{gy} f{f}"), &mut out);
                }
            }
        }
        mark.set_gaze(None);
        dir.set_resting(&mut mark, false);
        for (state, _) in super::super::clips::STATES {
            dir.set_state_pub(&mut mark, state);
            for f in 0..75 {
                step(&mut dir, &mut mark);
                if f % 3 == 0 {
                    record(&mark, format!("{state} f{f}"), &mut out);
                }
            }
        }
        for kind in super::super::UN {
            mark.poke(kind, 1.0);
            for f in 0..45 {
                step(&mut dir, &mut mark);
                record(&mark, format!("poke {kind} f{f}"), &mut out);
            }
        }
        out
    }

    /// Wherever the kit cloud (the loft body custom bodies are built like)
    /// shows two eyes, a custom body does too, through the same resting
    /// gaze sweep, every state clip and every poke, at every avatar size;
    /// and at rest it shows them in every frame.
    #[test]
    fn eyes_in_every_frame_the_kit_has_them() {
        for path in [CAT, RECEIPT] {
            let name = custom_body(path).unwrap();
            for (extent, display) in SIZES {
                let kit = eye_counts("cloud", extent, display);
                let ours = eye_counts(name, extent, display);
                let mut missing = Vec::new();
                for ((label, k), (_, o)) in kit.iter().zip(&ours) {
                    if label.starts_with("rest") {
                        assert_eq!(*o, Some(2), "{name} at {display}: {label}");
                    }
                    if *k == Some(2) && *o != Some(2) {
                        missing.push(label.clone());
                    }
                }
                let shown = ours.iter().filter(|(_, o)| *o == Some(2)).count();
                let drawn = ours.iter().filter(|(_, o)| o.is_some()).count();
                println!("{name} at {display}: eyes in {shown}/{drawn} frames, kit-only {}", missing.len());
                assert!(missing.is_empty(), "{name} at {display}: no eyes in {missing:?}");
            }
        }
    }

    /// Eye points lie inside the body outline in every frame of the run
    /// (within 1.5px of a 144px sprite: turned far, an eye laid on the
    /// dome's tangent plane can graze the limb, as the kit cloud's can; the
    /// cutout mask trims it).
    #[test]
    fn eyes_stay_inside_the_body() {
        let tab = shared_tables();
        for path in [CAT, RECEIPT] {
            let body = custom_body(path).unwrap();
            let mut mark = Mark::new(&tab, Some(body), 1.0, true, false, 3);
            let mut dir = Director::new(&mut mark, "curious", 3, false, false, None);
            let gazes = [[0.0, 0.0], [0.9, 0.3], [-0.8, -0.6], [0.4, 0.9], [-0.5, 0.2]];
            for (g, gaze) in gazes.iter().enumerate() {
                mark.set_gaze(Some(*gaze));
                for f in 0..45 {
                    step(&mut dir, &mut mark);
                    if f % 5 != 4 || mark.frame().morph.is_some() {
                        continue;
                    }
                    let (bodies, eyes) = body_and_eyes(&render(&mark));
                    let outline = &bodies[0];
                    for p in eyes.iter().flatten() {
                        assert!(
                            signed_distance(outline, *p) > -1.5,
                            "{body} gaze {g} frame {f}: eye point {p:?} outside"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn morph_kit_custom_kit() {
        let tab = shared_tables();
        let cat = custom_body(CAT).unwrap();
        let receipt = custom_body(RECEIPT).unwrap();
        let mut mark = Mark::new(&tab, Some("square"), 1.0, false, false, 11);
        let mut dir = Director::new(&mut mark, "happy", 11, false, false, None);
        for target in [cat, "hex", receipt, cat, "sphere", receipt, "house"] {
            mark.set_body(target, false);
            let mut blended = 0;
            for _ in 0..45 {
                step(&mut dir, &mut mark);
                let frame = mark.frame();
                if frame.bodies.len() > 1 {
                    blended += 1;
                }
                let tree = render(&mark);
                let mut d = Vec::new();
                paths(&tree, &|_| true, false, &mut d);
                assert!(
                    d.iter().any(|d| loops(d).iter().any(|l| l.len() >= 3)),
                    "no body drawn morphing to {target}"
                );
            }
            assert!(blended > 0, "no morph frames into {target}");
            assert_eq!(mark.frame().bodies[0].body, target);
        }
    }

    fn xml(t: &MarkTree, out: &mut String) {
        *out += &format!("<{}", t.tag);
        for (k, v) in &t.attrs {
            *out += &format!(" {k}=\"{v}\"");
        }
        *out += ">";
        for c in &t.children {
            xml(c, out);
        }
        *out += &format!("</{}>", t.tag);
    }

    /// A resting frame as an SVG group in a `px` square at (`x`, `y`).
    fn rest_svg(bytes: &[u8], px: f64, x: f64, y: f64, ink: &str) -> String {
        let rings = super::super::paint::rest_rings(bytes, &Default::default(), px);
        let mut d = String::new();
        for ring in rings.iter() {
            for (i, p) in ring.iter().enumerate() {
                d += &format!("{}{:.2} {:.2}", if i == 0 { "M" } else { "L" }, p[0] + x, p[1] + y);
            }
            d += "Z";
        }
        format!("<path d='{d}' fill='{ink}' fill-rule='evenodd'/>")
    }

    /// A body at a pose (pitch, yaw, roll degrees), drawn by `uh` at a
    /// large extent (fine angle steps) and scaled into a `px` square.
    fn pose_svg(body: &'static str, rot: V3, px: f64, x: f64, y: f64, id: &str) -> String {
        let tab = shared_tables();
        let mut mark = Mark::new(&tab, Some(body), 1.0, false, false, 1);
        mark.u = rot;
        let big = 1200.0;
        let tree = uh(
            &mark.frame(),
            &PaintOpts {
                cutout: true,
                ink_paint: Some("#1884FC".to_owned()),
                ..Default::default()
            },
            big,
            id,
            &UhSize {
                display_size: Some(big * 0.6 / 0.92),
                extent: big,
            },
            &tab,
        );
        let mut svg = String::new();
        xml(&tree, &mut svg);
        format!("<g transform='translate({x} {y}) scale({})'>{svg}</g>", px / big)
    }

    fn label(x: f64, y: f64, text: &str) -> String {
        format!("<text x='{x}' y='{y}' font-family='Helvetica' font-size='12' fill='#555'>{text}</text>")
    }

    /// Writes the round-2 comparison sheets as SVG into `MARK_RENDER_DIR`
    /// (convert with any SVG rasterizer); does nothing without it.
    #[test]
    fn render_sheets() {
        let Ok(dir) = std::env::var("MARK_RENDER_DIR") else {
            return;
        };
        let cat = custom_body(CAT).unwrap();
        let receipt = custom_body(RECEIPT).unwrap();
        let rows: [(&str, &'static str, String); 6] = [
            ("blob", "sphere", "blob".into()),
            ("square", "square", "square".into()),
            ("cloud", "cloud", "cloud".into()),
            ("heart", "heartChubby", "heart".into()),
            ("cat", cat, format!("outline:{CAT}")),
            ("receipt", receipt, format!("outline:{RECEIPT}")),
        ];
        let head = |w: f64, h: f64| {
            format!("<svg xmlns='http://www.w3.org/2000/svg' width='{w}' height='{h}'><rect width='100%' height='100%' fill='#fff'/>")
        };
        // rest: what paused cells and resting avatars paint, at 3 sizes
        let mut svg = head(560.0, 6.0 * 150.0 + 30.0);
        for (c, pt) in [64.0, 80.0, 96.0].iter().enumerate() {
            svg += &label(130.0 + c as f64 * 150.0, 18.0, &format!("rest {pt}pt"));
        }
        for (r, (name, _, shape)) in rows.iter().enumerate() {
            let y = 30.0 + r as f64 * 150.0;
            svg += &label(8.0, y + 70.0, name);
            for (c, pt) in [64.0, 80.0, 96.0].iter().enumerate() {
                let extent = pt * 0.82 * 0.92 / 0.6;
                let bytes = super::super::paint::rest_bytes(shape).unwrap();
                svg += &rest_svg(&bytes, extent, 110.0 + c as f64 * 150.0 + (150.0 - extent) / 2.0, y + (150.0 - extent) / 2.0, "#1884FC");
            }
        }
        std::fs::write(format!("{dir}/rest.svg"), svg + "</svg>").unwrap();
        // turning: yaw -40/0/40, a side view and a look from above
        let poses: [(&str, V3); 6] = [
            ("yaw -40", [0.0, -40.0, 0.0]),
            ("front", [0.0, 0.0, 0.0]),
            ("yaw 40", [0.0, 40.0, 0.0]),
            ("yaw 72", [0.0, 72.0, 0.0]),
            ("yaw 88", [0.0, 88.0, 0.0]),
            ("pitch 40", [40.0, 20.0, 0.0]),
        ];
        let mut svg = head(110.0 + 6.0 * 150.0, 6.0 * 150.0 + 30.0);
        for (c, (title, _)) in poses.iter().enumerate() {
            svg += &label(140.0 + c as f64 * 150.0, 18.0, title);
        }
        for (r, (name, body, _)) in rows.iter().enumerate() {
            let y = 30.0 + r as f64 * 150.0;
            svg += &label(8.0, y + 70.0, name);
            for (c, (_, rot)) in poses.iter().enumerate() {
                svg += &pose_svg(body, *rot, 150.0, 100.0 + c as f64 * 150.0, y, &format!("t{r}x{c}"));
            }
        }
        std::fs::write(format!("{dir}/turn.svg"), svg + "</svg>").unwrap();
        // live idle at the avatar sizes (the engine's own angle snapping)
        let tab = shared_tables();
        let mut svg = head(560.0, 6.0 * 150.0 + 30.0);
        for (c, pt) in [64.0, 80.0, 96.0].iter().enumerate() {
            svg += &label(130.0 + c as f64 * 150.0, 18.0, &format!("live {pt}pt"));
        }
        for (r, (name, body, _)) in rows.iter().enumerate() {
            let y = 30.0 + r as f64 * 150.0;
            svg += &label(8.0, y + 70.0, name);
            for (c, pt) in [64.0, 80.0, 96.0].iter().enumerate() {
                let extent = pt * 0.82 * 0.92 / 0.6;
                let mut mark = Mark::new(&tab, Some(body), 1.0, false, false, 4);
                let mut dir = Director::new(&mut mark, "happy", 4, false, false, None);
                for _ in 0..40 {
                    step(&mut dir, &mut mark);
                }
                let tree = uh(
                    &mark.frame(),
                    &PaintOpts {
                        cutout: true,
                        ink_paint: Some("#1884FC".to_owned()),
                        ..Default::default()
                    },
                    extent,
                    &format!("l{r}x{c}"),
                    &UhSize {
                        display_size: Some(pt * 0.82),
                        extent,
                    },
                    &tab,
                );
                let mut t = String::new();
                xml(&tree, &mut t);
                svg += &format!(
                    "<g transform='translate({} {})'>{t}</g>",
                    110.0 + c as f64 * 150.0 + (150.0 - extent) / 2.0,
                    y + (150.0 - extent) / 2.0
                );
            }
        }
        std::fs::write(format!("{dir}/live.svg"), svg + "</svg>").unwrap();
    }

    // -- rich bodies ---------------------------------------------------------

    /// (a) a green brute: spiky hair in the outline, a dark fringe, a cream
    /// face patch, faint blush, bright green eyes.
    const HULK: &str = r##"{"o":"M14 54 C14 34 24 24 34 19 L36 5 L45 16 L53 3 L60 16 L70 7 L70 21 C81 28 88 40 88 55 C88 80 72 96 50 96 C28 96 14 80 14 54 Z","c":"#5E9A2E","l":[["M16 42 C18 27 31 19 50 19 C69 19 83 27 85 42 C76 35 67 33 60 37 C54 31 45 31 39 37 C31 33 23 35 16 42 Z","#2B2019",1],["M22 62 C22 48 34 41 50 41 C66 41 79 48 79 62 C79 81 67 92 50 92 C33 92 22 81 22 62 Z","#F6EBD6",1],["M25 72 C25 68 34 68 34 72 C34 76 25 76 25 72 Z M66 72 C66 68 75 68 75 72 C75 76 66 76 66 72 Z","#F2B8A8",0.55]],"e":"#7CE03C"}"##;
    /// (b) a red masked hero: two black eye patches, white eyes.
    const HERO: &str = r##"{"o":"M50 4 C76 4 90 26 90 52 C90 80 72 96 50 96 C28 96 10 80 10 52 C10 26 24 4 50 4 Z","c":"#D62828","l":[["M25 46 C29 37 43 37 49 47 C45 56 31 58 25 46 Z M75 46 C71 37 57 37 51 47 C55 56 69 58 75 46 Z","#111111",1]],"e":"#FFFFFF"}"##;
    /// (c) an orange cat: a cream muzzle on the lower face, dark eyes.
    const ORANGE_CAT: &str = r##"{"o":"M20 92 C8 80 8 55 14 40 L10 6 L38 26 C46 23 54 23 62 26 L90 6 L86 40 C92 55 92 80 80 92 C64 100 36 100 20 92 Z","c":"#F28C28","l":[["M31 79 C31 69 42 66 50 70 C58 66 69 69 69 79 C69 88 59 92 50 90 C41 92 31 88 31 79 Z","#FBE9CF",1]],"e":"#2A1E14"}"##;

    fn render_ink(mark: &Mark, extent: f64, display: f64, ink: &str) -> MarkTree {
        let tab = shared_tables();
        uh(
            &mark.frame(),
            &PaintOpts {
                cutout: true,
                ink_paint: Some(ink.to_owned()),
                ..Default::default()
            },
            extent,
            "m",
            &UhSize {
                display_size: Some(display),
                extent,
            },
            &tab,
        )
    }

    /// Every path of a tree, with the decal layer it draws (if any).
    fn all_paths<'a>(t: &'a MarkTree, out: &mut Vec<&'a MarkTree>) {
        if t.tag == "path" {
            out.push(t);
        }
        if t.tag == "defs" {
            return;
        }
        for c in &t.children {
            all_paths(c, out);
        }
    }

    /// (decal layers drawn, eye paths drawn, body loop) of a rich tree.
    fn rich_parts(t: &MarkTree, eye_fill: &str) -> (Vec<usize>, usize, Vec<V2>, Vec<V2>) {
        // (eye paths carry `data-eye`, decal paths `data-layer`)
        let mut ps = Vec::new();
        all_paths(t, &mut ps);
        let layers: Vec<usize> = ps
            .iter()
            .filter_map(|p| p.attr("data-layer").and_then(|l| l.parse().ok()))
            .collect();
        let eyes = ps.iter().filter(|p| p.attr("data-eye").is_some()).count();
        if let Some(e) = ps.iter().find(|p| p.attr("data-eye").is_some()) {
            let fill = e.attr("fill").unwrap_or("");
            assert!(fill == eye_fill || fill.starts_with("rgb(") || eye_fill.is_empty(), "eye fill {fill}");
        }
        let body = ps.first().map(|p| loops(p.attr("d").unwrap_or("")).remove(0)).unwrap_or_default();
        let decal_pts = ps
            .iter()
            .filter(|p| p.attr("data-layer").is_some())
            .flat_map(|p| loops(p.attr("d").unwrap_or("")))
            .flatten()
            .collect();
        (layers, eyes, body, decal_pts)
    }

    #[test]
    fn rich_body_names_and_errors() {
        let a = rich_body(HULK).unwrap();
        assert!(a.starts_with("rich:"));
        assert_eq!(rich_body(HULK).unwrap(), a);
        assert_eq!(rich_body(&format!(" {HULK}\n")).unwrap(), a);
        assert_ne!(rich_body(HERO).unwrap(), a);
        assert_eq!(yn(a), a);
        // the geometry is the plain custom body's
        let recipe: serde_json::Value = serde_json::from_str(HULK).unwrap();
        let plain = custom_body(recipe["o"].as_str().unwrap()).unwrap();
        assert!(Rc::ptr_eq(&find(a).unwrap(), &find(plain).unwrap()));
        assert!(super::super::paint::rest_bytes(a).is_none());
        let nine: Vec<String> = (0..9).map(|_| r##"["M10 10 L20 10 L20 20 Z","#000000"]"##.to_owned()).collect();
        for bad in [
            "",
            "{",
            "[]",
            r##"{"c":"#FF0000","e":"#000000"}"##,
            r##"{"o":"M0 0 L10 10","c":"#FF0000","e":"#000000"}"##,
            r##"{"o":"M10 10 L90 10 L90 90 Z","c":"red","e":"#000000"}"##,
            r##"{"o":"M10 10 L90 10 L90 90 Z","c":"#FF0000"}"##,
            r##"{"o":"M10 10 L90 10 L90 90 Z","c":"#FF0000","e":"#000","l":[["M0 0 A1 1 0 0 0 5 5 Z","#FFFFFF"]]}"##,
            r##"{"o":"M10 10 L90 10 L90 90 Z","c":"#FF0000","e":"#000","l":[["M0 0 L10 10","#FFFFFF"]]}"##,
            r##"{"o":"M10 10 L90 10 L90 90 Z","c":"#FF0000","e":"#000","l":[["M10 10 L20 10 L20 20 Z","#FFFFFF","x"]]}"##,
            &format!(r##"{{"o":"M10 10 L90 10 L90 90 Z","c":"#FF0000","e":"#000","l":[{}]}}"##, nine.join(",")),
        ] {
            assert!(rich_body(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    /// The avatar's ink (the colour picker) changes nothing on a rich body.
    #[test]
    fn rich_ignores_the_ink() {
        let tab = shared_tables();
        let name = rich_body(HULK).unwrap();
        let mut a = 0xcbf29ce484222325u64;
        let mut b = a;
        let mut mark = Mark::new(&tab, Some(name), 1.0, false, false, 2);
        let mut dir = Director::new(&mut mark, "thinking", 2, false, false, None);
        for f in 0..90 {
            step(&mut dir, &mut mark);
            if f % 5 == 0 {
                hash_tree(&render_ink(&mark, 120.71, 78.72, "#1884FC"), &mut a);
                hash_tree(&render_ink(&mark, 120.71, 78.72, "#FC2C3E"), &mut b);
            }
        }
        assert_eq!(a, b);
        let mut ps = Vec::new();
        let tree = render_ink(&mark, 120.71, 78.72, "#1884FC");
        all_paths(&tree, &mut ps);
        assert!(ps.iter().all(|p| p.attr("fill") != Some("#1884FC")));
    }

    /// Hover darkening reaches every colour of a rich body.
    #[test]
    fn rich_darkens_with_the_edit_shade() {
        let tab = shared_tables();
        let name = rich_body(HULK).unwrap();
        let mark = Mark::new(&tab, Some(name), 1.0, false, false, 2);
        let plain = render_ink(&mark, 120.71, 78.72, "#1884FC");
        let dark = render_ink(&mark, 120.71, 78.72, &super::super::darken("#1884FC", 0.35));
        let fills = |t: &MarkTree| {
            let mut ps = Vec::new();
            all_paths(t, &mut ps);
            ps.iter().filter_map(|p| p.attr("fill").map(str::to_owned)).collect::<Vec<_>>()
        };
        let (pf, df) = (fills(&plain), fills(&dark));
        assert_eq!(pf.len(), df.len());
        assert!(pf.contains(&"#5E9A2E".to_owned()));
        for (p, d) in pf.iter().zip(&df) {
            assert_eq!(*d, super::super::darken(p, 0.35), "{p}");
        }
    }

    fn frame_eyes(mark: &Mark) -> bool {
        mark.frame().eyes.is_some()
    }

    fn frame_morph(mark: &Mark) -> bool {
        mark.frame().morph.is_some()
    }

    /// One avatar run (rest gaze sweep with the edit shade, every state
    /// clip, the pokes): each frame's tree, whether the frame has eyes,
    /// whether a status morph is on.
    fn rich_run(body: &'static str, extent: f64, display: f64) -> Vec<(String, MarkTree, bool, bool)> {
        let tab = shared_tables();
        let mut out = Vec::new();
        let mut mark = Mark::new(&tab, Some(body), 1.0, false, false, 9);
        let mut dir = Director::new(&mut mark, "idle", 9, display < 48.0, true, None);
        let edit = super::super::darken("#1884FC", 0.7);
        for gy in -2..=2 {
            for gx in -2..=2 {
                mark.set_gaze(Some([gx as f64 / 2.0, gy as f64 / 2.0]));
                for f in 0..10 {
                    mark.advance(1.0 / 30.0);
                    let ink = if (gx + gy) % 2 == 0 { "#1884FC" } else { edit.as_str() };
                    out.push((format!("rest gaze {gx},{gy} f{f}"), render_ink(&mark, extent, display, ink), frame_eyes(&mark), frame_morph(&mark)));
                }
            }
        }
        mark.set_gaze(None);
        dir.set_resting(&mut mark, false);
        for (state, _) in super::super::clips::STATES {
            dir.set_state_pub(&mut mark, state);
            for f in 0..75 {
                step(&mut dir, &mut mark);
                if f % 3 == 0 {
                    out.push((format!("{state} f{f}"), render_ink(&mark, extent, display, "#1884FC"), frame_eyes(&mark), frame_morph(&mark)));
                }
            }
        }
        for kind in super::super::UN {
            mark.poke(kind, 1.0);
            for f in 0..45 {
                step(&mut dir, &mut mark);
                out.push((format!("poke {kind} f{f}"), render_ink(&mark, extent, display, "#1884FC"), frame_eyes(&mark), frame_morph(&mark)));
            }
        }
        out
    }

    /// Wherever the eyes are drawn the decals are too (the face patch, in
    /// every such frame), and in the frames a status morph takes the eyes
    /// away (`frame.eyes` is None) the decals go with them. (Turned far
    /// enough that the eyes face away, a sliver of patch can still show at
    /// the limb without them, as it would on a real head.)
    #[test]
    fn decals_follow_the_eyes() {
        for (recipe, patch, eye) in [(HULK, 1usize, "#7CE03C"), (ORANGE_CAT, 0, "#2A1E14")] {
            let name = rich_body(recipe).unwrap();
            for (extent, display) in [(80.47, 52.48), (120.71, 78.72), (50.29, 32.8)] {
                let (mut with_eyes, mut frames) = (0, 0);
                for (label, tree, has_eyes, _) in rich_run(name, extent, display) {
                    let (layers, eyes, _, _) = rich_parts(&tree, eye);
                    frames += 1;
                    if eyes > 0 {
                        with_eyes += 1;
                        assert!(layers.contains(&patch), "{name} at {display}: {label}: layers {layers:?}");
                    }
                    if !has_eyes {
                        assert!(layers.is_empty(), "{name} at {display}: {label}: decals without eyes");
                    }
                    if label.starts_with("rest") {
                        assert!(eyes >= 2, "{name} at {display}: {label}: {eyes} eyes");
                    }
                }
                println!("{name} at {display}: eyes and decals in {with_eyes}/{frames} frames");
            }
        }
    }

    /// Decal points lie inside the body outline (within 1.5px at the 96pt
    /// avatar) through the run, at every pose it takes. The slack is the
    /// atlas: between its 10° nodes the outline is a blend of neighbouring
    /// silhouettes, a hair inside the true one; the clip trims the rest.
    #[test]
    fn decals_stay_inside_the_body() {
        for recipe in [HULK, HERO, ORANGE_CAT] {
            let name = rich_body(recipe).unwrap();
            let mut worst = f64::INFINITY;
            for (label, tree, _, morph) in rich_run(name, 120.71, 78.72) {
                let (_, _, body, decals) = rich_parts(&tree, "");
                // a status morph reshapes the body ring under the decals;
                // the clip keeps them inside there
                if body.len() < 3 || morph {
                    continue;
                }
                for p in decals {
                    let d = signed_distance(&body, p);
                    worst = worst.min(d);
                    assert!(d > -1.5, "{name}: {label}: decal point {p:?} {d:.2}px outside");
                }
            }
            println!("{name}: worst decal point {worst:.2}px from the outline");
        }
    }

    /// Debug-build cost of a rich frame against the plain body's (printed).
    #[test]
    fn rich_frame_timing() {
        let tab = shared_tables();
        for (label, body) in [("plain", custom_body(CAT).unwrap()), ("rich", rich_body(ORANGE_CAT).unwrap()), ("rich brute", rich_body(HULK).unwrap())] {
            let mut mark = Mark::new(&tab, Some(body), 1.0, false, false, 2);
            let mut dir = Director::new(&mut mark, "curious", 2, false, false, None);
            // warm the atlas nodes the run visits
            for _ in 0..60 {
                step(&mut dir, &mut mark);
                render_ink(&mark, 120.71, 78.72, "#1884FC");
            }
            let t = std::time::Instant::now();
            for _ in 0..60 {
                step(&mut dir, &mut mark);
                render_ink(&mark, 120.71, 78.72, "#1884FC");
            }
            println!("{label}: {:?} per frame", t.elapsed() / 60);
        }
    }

    /// Plain custom bodies keep their geometry when a rich body shares it.
    #[test]
    fn rich_shares_the_plain_body() {
        let name = rich_body(ORANGE_CAT).unwrap();
        let plain = custom_body(CAT).unwrap();
        assert!(Rc::ptr_eq(&bodies::atlas_of(name).unwrap(), &bodies::atlas_of(plain).unwrap()));
    }

    /// Writes the rich-body sheets as SVG into `MARK_RICH_DIR` (convert
    /// with any SVG rasterizer); does nothing without it.
    #[test]
    fn render_rich_sheets() {
        let Ok(dir) = std::env::var("MARK_RICH_DIR") else {
            return;
        };
        let tab = shared_tables();
        let rows: Vec<(&str, &'static str)> = vec![
            ("blob", "sphere"),
            ("brute", rich_body(HULK).unwrap()),
            ("hero", rich_body(HERO).unwrap()),
            ("cat", rich_body(ORANGE_CAT).unwrap()),
        ];
        let head = |w: f64, h: f64| {
            format!("<svg xmlns='http://www.w3.org/2000/svg' width='{w}' height='{h}'><rect width='100%' height='100%' fill='#fff'/>")
        };
        let tree_svg = |tree: &MarkTree, x: f64, y: f64, scale: f64| {
            let mut t = String::new();
            xml(tree, &mut t);
            format!("<g transform='translate({x} {y}) scale({scale})'>{t}</g>")
        };
        let ink = "#1884FC";
        // rest at the avatar sizes, live at the exact rest pose
        let mut svg = head(560.0, rows.len() as f64 * 150.0 + 30.0);
        for (c, pt) in [64.0, 80.0, 96.0].iter().enumerate() {
            svg += &label(130.0 + c as f64 * 150.0, 18.0, &format!("rest {pt}pt"));
        }
        for (r, (name, body)) in rows.iter().enumerate() {
            let y = 30.0 + r as f64 * 150.0;
            svg += &label(8.0, y + 70.0, name);
            for (c, pt) in [64.0, 80.0, 96.0].iter().enumerate() {
                let extent = pt * 0.82 * 0.92 / 0.6;
                let (x0, y0) = (110.0 + c as f64 * 150.0 + (150.0 - extent) / 2.0, y + (150.0 - extent) / 2.0);
                if *body == "sphere" {
                    let bytes = super::super::paint::rest_bytes("blob").unwrap();
                    svg += &rest_svg(&bytes, extent, x0, y0, ink);
                } else {
                    let mark = Mark::new(&tab, Some(body), 1.0, false, false, 1);
                    let mut t = render_ink(&mark, extent, pt * 0.82, ink);
                    t.attrs.retain(|(k, _)| *k != "id");
                    svg += &tree_svg(&t, x0, y0, 1.0);
                }
            }
        }
        std::fs::write(format!("{dir}/rest.svg"), svg + "</svg>").unwrap();
        // poses
        let poses: [(&str, V3); 6] = [
            ("yaw -40", [0.0, -40.0, 0.0]),
            ("front", [0.0, 0.0, 0.0]),
            ("yaw 40", [0.0, 40.0, 0.0]),
            ("yaw 72", [0.0, 72.0, 0.0]),
            ("yaw 88", [0.0, 88.0, 0.0]),
            ("pitch 40", [40.0, 20.0, 0.0]),
        ];
        let mut svg = head(110.0 + 6.0 * 150.0, rows.len() as f64 * 150.0 + 30.0);
        for (c, (title, _)) in poses.iter().enumerate() {
            svg += &label(140.0 + c as f64 * 150.0, 18.0, title);
        }
        for (r, (name, body)) in rows.iter().enumerate() {
            let y = 30.0 + r as f64 * 150.0;
            svg += &label(8.0, y + 70.0, name);
            for (c, (_, rot)) in poses.iter().enumerate() {
                svg += &pose_svg(body, *rot, 150.0, 100.0 + c as f64 * 150.0, y, &format!("p{r}x{c}"));
            }
        }
        std::fs::write(format!("{dir}/turn.svg"), svg + "</svg>").unwrap();
        // a status morph coming on: the eyes and decals fade out together
        let times = [0.0, 0.05, 0.1, 0.2, 0.5, 1.2];
        let mut svg = head(110.0 + times.len() as f64 * 150.0, rows.len() as f64 * 150.0 + 30.0);
        for (c, t) in times.iter().enumerate() {
            svg += &label(130.0 + c as f64 * 150.0, 18.0, &format!("morph +{t}s"));
        }
        for (r, (name, body)) in rows.iter().enumerate() {
            let y = 30.0 + r as f64 * 150.0;
            svg += &label(8.0, y + 70.0, name);
            let mut mark = Mark::new(&tab, Some(body), 1.0, false, false, 1);
            mark.set_morph(Some("thinking"), false);
            let mut clock = 0.0;
            for (c, t) in times.iter().enumerate() {
                while clock < *t - 1e-9 {
                    mark.advance(1.0 / 60.0);
                    clock += 1.0 / 60.0;
                }
                let tree = render_ink(&mark, 144.0, 96.0, ink);
                svg += &tree_svg(&tree, 100.0 + c as f64 * 150.0, y, 150.0 / 144.0);
            }
        }
        std::fs::write(format!("{dir}/morph.svg"), svg + "</svg>").unwrap();
    }
}
