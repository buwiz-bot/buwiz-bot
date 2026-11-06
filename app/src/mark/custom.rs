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
use super::{render, shared_tables, solids, Mark, MN, V2, V3};

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
/// The kit square's body size (`hn(1.0413, ..)`): a custom body's eyes are
/// sized against it.
const SQUARE_SIZE: f64 = 1.0413;
/// How much of an avatar's frame a custom figure fills, in body units at size 1: its height, its
/// width, and how far any point may reach from the middle. Every figure stands as tall as the
/// others, whatever it wears, unless it's so wide or so far-reaching that it would leave the frame
/// as it turns.
const FIT_HEIGHT: f64 = 3.33;
const FIT_WIDTH: f64 = 3.53;
///
/// The reach leaves room inside the canvas for what the states do: `nothing_leaves_the_canvas`
/// holds it to that.
const FIT_REACH: f64 = 2.1;
/// The largest a custom figure draws.
const MAX_SIZE: f64 = 2.4;
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
/// The loft's center for an outline (`sS` scales its rings about the
/// bounding box's).
fn loft_mid(poly: &[V2]) -> V2 {
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
    [(x0 + x1) / 2.0, (y0 + y1) / 2.0]
}

/// Whether two eye ovals `w`×`h`, `gap` apart, centred on `at`, clear the
/// outline by `margin` (with an expression's slack) and stay inside
/// [`EYE_REACH`] of the loft's center `mid`.
fn eyes_clear(poly: &[V2], mid: V2, at: V2, w: f64, h: f64, gap: f64, margin: f64) -> bool {
    [-1.0, 1.0].iter().all(|side| {
        (0..32).all(|i| {
            let t = TAU * i as f64 / 32.0;
            let p = [
                at[0] + side * gap / 2.0 + t.cos() * w * 0.6,
                at[1] + t.sin() * h * 0.6,
            ];
            let reach = [
                mid[0] + (p[0] - mid[0]) / EYE_REACH,
                mid[1] + (p[1] - mid[1]) / EYE_REACH,
            ];
            signed_distance(poly, p) >= margin && signed_distance(poly, reach) >= 0.0
        })
    })
}

fn place_eyes(poly: &[V2], size: f64, (c, r): (V2, f64)) -> (AvEye, f64) {
    let mid = loft_mid(poly);
    let y = c[1] + EYE_RISE_PER_R * r;
    let base = SQUARE_SIZE / size;
    let mut k = 1.0;
    for _ in 0..40 {
        let [w, h, gap] = SQUARE_EYE.map(|v| v * base * k);
        // the oval at full open plus a widened/taller expression's slack
        if eyes_clear(poly, mid, [c[0], y], w, h, gap, EYE_MARGIN * k) {
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
#[derive(Clone, Copy)]
struct FrontMap {
    scale: f64,
    center: V2,
    /// `Vv`'s `(k, dx, dy)`, or identity.
    renorm: [f64; 3],
}

impl FrontMap {
    /// A point of the 100×100 input box (y down) in body units (y up).
    fn to_body(self, p: V2) -> V2 {
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
    let size = fit_size(poly.iter().map(|p| [p[0], p[1], 0.0]));
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

/// How far past the outline (body units) a decal point counts as off the
/// body; points nearer the rim ride it.
const OFF_BODY: f64 = 0.02;

/// One decal layer of a rich body: closed loops on the body's front (body
/// units, y up, densely sampled so they bend with the surface), a fill and
/// an opacity.
pub(crate) struct Decal {
    pub loops: Vec<Vec<V2>>,
    /// Per point: past the outline by more than [`OFF_BODY`].
    pub off: Vec<Vec<bool>>,
    /// Drawn over the eyes and rims (a fringe, an eyepatch, glasses).
    pub over: bool,
    pub fill: String,
    pub opacity: f64,
}

/// A rich body's paint: the shell, the decals back to front, the eyes.
pub(crate) struct Rich {
    pub shell: String,
    pub eyes: String,
    pub decals: Vec<Decal>,
    /// The recipe's own eye box and eye midpoint x (body units), when its
    /// `"e"` places or sizes the eyes; else the outline's computed ones.
    pub eye_box: Option<(AvEye, f64)>,
    /// A rim around each eye: its colour and thickness (body units).
    pub rim: Option<(String, f64)>,
    /// The eye drawn always shut (0 the viewer's left, 1 the right).
    pub shut: Option<usize>,
    /// Eyes under an over-layer at design time (viewer's left, right):
    /// not drawn at all, so no gaze shift, blink or wide expression can
    /// peek out past the layer's edge.
    pub hidden: [bool; 2],
    /// The extra parts (`"p"`), each its own lofted body in the main
    /// part's frame.
    pub parts: Vec<RichPart>,
    /// Placement for depth sorting: the main part, then each extra part.
    pub layout: Vec<Place>,
    /// Whether two parts' outlines overlap seen from the front (same
    /// indexing as `layout`).
    pub overlap: Vec<Vec<bool>>,
    /// The solid accessories (`"s"`), in head space.
    pub solids: Option<solids::Solids>,
}

/// An extra part of a rich body.
pub(crate) struct RichPart {
    /// The part's own body (registered under an internal `custom:` name).
    pub body: &'static str,
    pub geometry: Rc<CustomBody>,
    pub shell: String,
    pub decals: Vec<Decal>,
}

/// Where a part sits for depth sorting: its outline's area centroid on the
/// front plane (body units), its recipe bias `z` and the 3D offset toward
/// the viewer that bias gives it.
#[derive(Clone, Copy)]
pub(crate) struct Place {
    pub centroid: V2,
    pub z: f64,
    pub z_offset: f64,
}

/// The most extra parts a rich body takes.
const MAX_PARTS: usize = 6;
/// The thinnest an extra part's loft gets (half-depth, body units).
const PART_MIN_DEPTH: f64 = 0.06;
/// The range of a part's `d` (its half-depth in head half-depths).
const PART_DEPTH: (f64, f64) = (0.05, 1.6);

/// The largest subpath of an outline, in the 100×100 box.
fn outline_points(path: &str) -> Result<Vec<V2>, String> {
    let mut best: Option<(f64, Vec<V2>)> = None;
    for s in &subpaths(path)? {
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
    if area.is_nan() || area < 1.0 {
        return Err("the outline encloses no area".to_owned());
    }
    Ok(pts)
}

/// One part's lofted body, placed in the shared frame `front` at body
/// `size` (eyes are placed only on the main part), `depth` its loft
/// half-depth when the recipe sets one.
fn part_body(
    pts: &[V2],
    front: &FrontMap,
    size: f64,
    with_eyes: bool,
    depth: Option<f64>,
) -> Result<CustomBody, String> {
    let mut d = String::new();
    for (i, p) in pts.iter().enumerate() {
        let x = ((X + (p[0] - front.center[0]) * front.scale) * 100.0).round() / 100.0;
        let y = ((X + (p[1] - front.center[1]) * front.scale) * 100.0).round() / 100.0;
        d += &format!("{}{x} {y}", if i == 0 { "M" } else { "L" });
    }
    d += "Z";
    let outline = Rc::new(bodies::hc_raw(&d));
    let poly = body_polygon(&outline.path);
    if poly.len() < 3 || signed_area(&poly).abs() < 1e-3 {
        return Err("the outline encloses no area".to_owned());
    }
    let circle = inscribed_circle(&poly);
    let (eyes, eye_x) = if with_eyes {
        place_eyes(&poly, size, circle)
    } else {
        (
            AvEye {
                width: 0.0,
                height: 0.0,
                gap: 0.0,
                shift_y: 0.0,
            },
            0.0,
        )
    };
    // the head keeps a rounded body's thickness; a thin extra part (an
    // antler, a feather) stays as thin as it is wide
    let depth = match depth {
        Some(d) => d,
        None if with_eyes => loft_depth(circle.1),
        None => (CLOUD_DEPTH * circle.1).clamp(PART_MIN_DEPTH, CLOUD_DEPTH),
    };
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
        front: *front,
        size,
        loft_points,
        depth,
        eyes,
        eye_x,
        atlas,
        rest: std::cell::OnceCell::new(),
    })
}

/// The area centroid of a polygon.
fn centroid_of(poly: &[V2]) -> V2 {
    let n = poly.len();
    let (mut a, mut cx, mut cy) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        let w = p[0] * q[1] - q[0] * p[1];
        a += w;
        cx += (p[0] + q[0]) * w;
        cy += (p[1] + q[1]) * w;
    }
    if a.abs() < 1e-12 {
        return poly.first().copied().unwrap_or([0.0, 0.0]);
    }
    [cx / (3.0 * a), cy / (3.0 * a)]
}

/// Whether `p` is inside the polygon (even-odd).
fn inside(poly: &[V2], p: V2) -> bool {
    let n = poly.len();
    let mut c = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) {
            c = !c;
        }
    }
    c
}

/// Whether two outlines overlap (sampled on a 32×32 grid over their
/// common bounding box).
fn overlaps(a: &[V2], b: &[V2]) -> bool {
    let bbox = |p: &[V2]| {
        p.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |m, q| {
            [m[0].min(q[0]), m[1].min(q[1]), m[2].max(q[0]), m[3].max(q[1])]
        })
    };
    let (ba, bb) = (bbox(a), bbox(b));
    let (x0, y0, x1, y1) = (ba[0].max(bb[0]), ba[1].max(bb[1]), ba[2].min(bb[2]), ba[3].min(bb[3]));
    if x0 >= x1 || y0 >= y1 {
        return false;
    }
    const G: usize = 32;
    (0..G).any(|i| {
        (0..G).any(|j| {
            let p = [
                x0 + (x1 - x0) * (i as f64 + 0.5) / G as f64,
                y0 + (y1 - y0) * (j as f64 + 0.5) / G as f64,
            ];
            inside(a, p) && inside(b, p)
        })
    })
}

/// The main geometry, extra parts, their placement and the solids of a
/// multi-part recipe.
type MultiPart = (Rc<CustomBody>, Vec<RichPart>, Vec<Place>, Option<solids::Solids>);

/// The size that fills the frame with a figure of these points, in body units around its centre.
fn fit_size(points: impl Iterator<Item = V3>) -> f64 {
    let (mut x0, mut x1, mut y0, mut y1, mut far) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY, 0.0f64);
    for p in points {
        (x0, x1, y0, y1) = (x0.min(p[0]), x1.max(p[0]), y0.min(p[1]), y1.max(p[1]));
        far = far.max(p[0].hypot(p[1]).hypot(p[2]));
    }
    (FIT_HEIGHT / (y1 - y0)).min(FIT_WIDTH / (x1 - x0)).min(MAX_SIZE).min(FIT_REACH / far)
}

/// A solid mark's path (M/L/C/Q/Z, in its chart) as closed loops, sampled
/// every [`solids::MARK_STEP`]. Every coordinate is checked first: sampling
/// spends points by length.
fn chart_loops(path: &str) -> Result<Vec<Vec<V2>>, String> {
    let mut loops = Vec::new();
    for sub in subpaths(path)? {
        // absolute by now: every number is a coordinate, x then y
        let numbers = tokenize(&sub)?.into_iter().filter_map(|t| match t {
            Token::Number(v) => Some(v),
            Token::Command(_) => None,
        });
        for (i, v) in numbers.enumerate() {
            let range = if i % 2 == 0 { solids::CHART_U } else { solids::CHART_V };
            if !(range.0..=range.1).contains(&v) {
                return Err(format!("coordinates must stay within {}..{}, {}..{}", solids::CHART_U.0, solids::CHART_U.1, solids::CHART_V.0, solids::CHART_V.1));
            }
        }
        let pts = bodies::us(&sub, solids::MARK_STEP);
        if pts.len() >= 3 && signed_area(&pts).abs() >= 0.25 {
            loops.push(pts);
        }
    }
    Ok(loops)
}

/// A multi-part recipe's main geometry and extra parts: one frame from
/// the bounding box of every outline (the `Hv` fit of the whole figure),
/// one body size for all of them ([`fit_size`], solids included), each part
/// its own loft at its place in the frame.
fn multi_part(
    main: &str,
    spec: Option<&serde_json::Value>,
    solid_spec: Option<&serde_json::Value>,
    hash: u64,
) -> Result<MultiPart, String> {
    let list: &[serde_json::Value] = match spec {
        None => &[],
        Some(s) => s.as_array().ok_or("the parts (\"p\") must be a list")?,
    };
    if list.len() > MAX_PARTS {
        return Err(format!("at most {MAX_PARTS} parts, got {}", list.len()));
    }
    let main_pts = outline_points(main)?;
    let mut part_pts = Vec::with_capacity(list.len());
    for (i, part) in list.iter().enumerate() {
        let n = i + 1;
        if !part.is_object() {
            return Err(format!("part {n} must be an object"));
        }
        let o = part
            .get("o")
            .and_then(|o| o.as_str())
            .ok_or_else(|| format!("part {n} needs an outline (\"o\")"))?;
        part_pts.push(outline_points(o).map_err(|e| format!("part {n}: {e}"))?);
    }
    let (mut x0, mut x1, mut y0, mut y1) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for p in main_pts.iter().chain(part_pts.iter().flatten()) {
        x0 = x0.min(p[0]);
        x1 = x1.max(p[0]);
        y0 = y0.min(p[1]);
        y1 = y1.max(p[1]);
    }
    let specs = solid_spec
        .map(|s| solids::parse(s, &|v, what| colour(v, what), &chart_loops))
        .transpose()?;
    // solids count toward the frame as seen from the front
    let reach = specs.as_ref().map(|s| {
        let (mut hx0, mut hx1, mut hy0, mut hy1) =
            (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
        for p in &main_pts {
            hx0 = hx0.min(p[0]);
            hx1 = hx1.max(p[0]);
            hy0 = hy0.min(p[1]);
            hy1 = hy1.max(p[1]);
        }
        solids::reach(s, [hx0, hy0, hx1, hy1])
    });
    for p in reach.iter().flatten() {
        x0 = x0.min(p[0]);
        x1 = x1.max(p[0]);
        y0 = y0.min(p[1]);
        y1 = y1.max(p[1]);
    }
    let front = FrontMap {
        scale: BV / (x1 - x0).max(y1 - y0),
        center: [(x0 + x1) / 2.0, (y0 + y1) / 2.0],
        renorm: [1.0, 0.0, 0.0],
    };
    let unit = front.scale / X;
    // the solids sit on the head, whose shape doesn't change with its size
    let probe = part_body(&main_pts, &front, SQUARE_SIZE, true, None)?;
    let scene = specs.map(|s| solids::Solids::build(&s, &solids::Head::new(&probe.loft_points, probe.depth), unit, front.center));
    let c = front.center;
    let flat = main_pts.iter().chain(part_pts.iter().flatten()).map(|p| [(p[0] - c[0]) * unit, -(p[1] - c[1]) * unit, 0.0]);
    let size = fit_size(flat.chain(scene.iter().flat_map(|s| s.shown_points())));
    let base = Rc::new(part_body(&main_pts, &front, size, true, None)?);
    let mut parts = Vec::with_capacity(list.len());
    let mut layout = vec![Place {
        centroid: centroid_of(&body_polygon(&base.outline.path)),
        z: 0.0,
        z_offset: 0.0,
    }];
    for (i, (part, pts)) in list.iter().zip(&part_pts).enumerate() {
        let n = i + 1;
        // `d`: the part's half-depth in head half-depths (hair that wraps
        // the head is about as deep as it)
        let depth = match part.get("d") {
            None | Some(serde_json::Value::Null) => None,
            Some(d) => Some(
                d.as_f64()
                    .filter(|d| d.is_finite())
                    .ok_or_else(|| format!("part {n} \"d\" must be a number"))?
                    .clamp(PART_DEPTH.0, PART_DEPTH.1)
                    * base.depth,
            ),
        };
        let geometry =
            Rc::new(part_body(pts, &front, size, false, depth).map_err(|e| format!("part {n}: {e}"))?);
        let shell = colour(part.get("c"), &format!("part {n} shell"))?;
        let decals = parse_layers(part.get("l"), &geometry, &format!("part {n} "))?;
        let z = match part.get("z") {
            None | Some(serde_json::Value::Null) => 0.0,
            Some(z) => z
                .as_f64()
                .filter(|z| z.is_finite())
                .ok_or_else(|| format!("part {n} \"z\" must be a number"))?
                .clamp(-1.0, 1.0),
        };
        layout.push(Place {
            centroid: centroid_of(&body_polygon(&geometry.outline.path)),
            z,
            z_offset: z * base.depth,
        });
        let body: &'static str = Box::leak(format!("{PREFIX}{hash:016x}-part{n}").into_boxed_str());
        BODIES.with(|b| b.borrow_mut().insert(body, geometry.clone()));
        parts.push(RichPart {
            body,
            geometry,
            shell,
            decals,
        });
    }
    Ok((base, parts, layout, scene))
}

/// The order to draw a rich body's parts in at a pose, back to front, as
/// `layout` indices (0 is the main part). Two parts whose outlines overlap
/// from the front and whose `z` differ keep the recipe's layering (higher
/// `z` over lower) while the front faces the viewer, and swap once it
/// turns away; any other pair goes by depth: the rotated centroid, pushed
/// toward the viewer by the part's `z` offset.
pub(crate) fn part_order(rich: &Rich, rot: &bodies::Rot) -> Vec<usize> {
    let facing = bodies::view(rot, [0.0, 0.0, 1.0])[2] > 0.0;
    let depth: Vec<f64> = rich
        .layout
        .iter()
        .map(|p| bodies::view(rot, [p.centroid[0], p.centroid[1], p.z_offset])[2])
        .collect();
    // `a` draws before (behind) `b`
    let behind = |a: usize, b: usize| -> bool {
        let (za, zb) = (rich.layout[a].z, rich.layout[b].z);
        if rich.overlap[a][b] && za != zb {
            if facing {
                za < zb
            } else {
                za > zb
            }
        } else {
            depth[a] < depth[b]
        }
    };
    let mut order: Vec<usize> = (0..rich.layout.len()).collect();
    // an insertion sort: the comparison mixes two rules and needn't be a
    // total order, so place each part after the last one it sits in front of
    for i in 1..order.len() {
        let mut j = i;
        while j > 0 && behind(order[j], order[j - 1]) {
            order.swap(j, j - 1);
            j -= 1;
        }
    }
    order
}

/// The smallest and largest eye size multiplier a recipe may ask for.
const EYE_SCALE: (f64, f64) = (0.6, 2.2);

/// A number field of the recipe's eye object.
fn eye_number(e: &serde_json::Value, key: &str) -> Result<Option<f64>, String> {
    match e.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|v| v.is_finite())
            .map(Some)
            .ok_or_else(|| format!("eye \"{key}\" must be a number")),
    }
}

/// The eye box a recipe asks for (`at`/`gap`/`s` over the computed one),
/// fitted the way computed eyes are: the eyes shrink (spacing kept) toward
/// 0.6× while they cross the outline or the loft's steep rim, then move
/// toward the inscribed centre, then shrink with their spacing if they
/// still don't fit.
fn recipe_eyes(base: &CustomBody, at: Option<V2>, gap: Option<f64>, scale: Option<f64>) -> (AvEye, f64) {
    let poly = body_polygon(&base.outline.path);
    let (centre, _) = inscribed_circle(&poly);
    let mid = loft_mid(&poly);
    let mut c = match at {
        Some(p) => base.front.to_body(p),
        None => [base.eye_x, -base.eyes.shift_y.sin()],
    };
    let k = base.front.renorm[0];
    let unit = base.front.scale * k / X;
    let (mut w, mut h) = match scale {
        Some(s) => {
            let f = SQUARE_SIZE / base.size * s.clamp(EYE_SCALE.0, EYE_SCALE.1);
            (SQUARE_EYE[0] * f, SQUARE_EYE[1] * f)
        }
        None => (base.eyes.width, base.eyes.height),
    };
    let mut g = match gap {
        Some(g) => g.max(0.0) * unit,
        // bigger eyes than computed keep at least a quarter-eye between them
        None => base.eyes.gap.max(w * 1.25),
    };
    let floor = w * EYE_SCALE.0 / scale.unwrap_or(1.0).clamp(EYE_SCALE.0, EYE_SCALE.1);
    let start = c;
    let mut pull = 0.0;
    for _ in 0..120 {
        if eyes_clear(&poly, mid, c, w, h, g, EYE_MARGIN * 0.6) {
            break;
        }
        if w * 0.95 >= floor {
            // the size gives first; the spacing is the design's
            (w, h) = (w * 0.95, h * 0.95);
        } else if pull < 1.0 {
            pull = (pull + 0.1f64).min(1.0);
            c = [
                start[0] + (centre[0] - start[0]) * pull,
                start[1] + (centre[1] - start[1]) * pull,
            ];
        } else {
            (w, h, g) = (w * 0.95, h * 0.95, g * 0.95);
        }
    }
    (
        AvEye {
            width: w,
            height: h,
            gap: g,
            shift_y: (-c[1]).clamp(-0.9, 0.9).asin(),
        },
        c[0],
    )
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

/// A recipe's decal layers on `base`'s front (`part` prefixes the error
/// messages: empty for the main part).
fn parse_layers(layers: Option<&serde_json::Value>, base: &CustomBody, part: &str) -> Result<Vec<Decal>, String> {
    let layers = match layers {
        None | Some(serde_json::Value::Null) => &[][..],
        Some(serde_json::Value::Array(a)) => &a[..],
        Some(_) => return Err(format!("{part}the layers (\"l\") must be a list")),
    };
    if layers.len() > MAX_LAYERS {
        return Err(format!("{part}at most {MAX_LAYERS} layers, got {}", layers.len()));
    }
    let mut decals = Vec::with_capacity(layers.len());
    for (i, layer) in layers.iter().enumerate() {
        let n = format!("{part}layer {}", i + 1);
        // `[path, fill, opacity?, "over"?]`, or the object form
        // `{"path", "fill", "opacity"?, "over"?}`
        let (parts, over) = match layer {
            serde_json::Value::Array(a) => {
                let mut a = a.clone();
                // `[path, fill, "over"]` leaves the opacity out
                if a.get(2).and_then(|v| v.as_str()) == Some("over") {
                    a.insert(2, serde_json::Value::Null);
                }
                let over = match a.get(3) {
                    None | Some(serde_json::Value::Null) => false,
                    Some(serde_json::Value::String(s)) if s == "over" => true,
                    Some(_) => return Err(format!("{n}: the 4th entry may only be \"over\"")),
                };
                (a, over)
            }
            serde_json::Value::Object(o) => {
                let over = match o.get("over") {
                    None | Some(serde_json::Value::Null) => false,
                    Some(serde_json::Value::Bool(b)) => *b,
                    Some(_) => return Err(format!("{n}: \"over\" must be true or false")),
                };
                let mut a = vec![
                    o.get("path").cloned().unwrap_or(serde_json::Value::Null),
                    o.get("fill").cloned().unwrap_or(serde_json::Value::Null),
                ];
                if let Some(op) = o.get("opacity") {
                    a.push(op.clone());
                }
                (a, over)
            }
            _ => return Err(format!("{n} must be [path, colour, opacity]")),
        };
        let path = parts
            .first()
            .and_then(|p| p.as_str())
            .ok_or_else(|| format!("{n} needs a path"))?;
        let fill = colour(parts.get(1), &n)?;
        let opacity = match parts.get(2) {
            None | Some(serde_json::Value::Null) => 1.0,
            Some(o) => o
                .as_f64()
                .filter(|o| o.is_finite())
                .ok_or_else(|| format!("{n} opacity must be a number"))?
                .clamp(0.0, 1.0),
        };
        let poly = body_polygon(&base.outline.path);
        let mut loops = Vec::new();
        for sub in subpaths(path).map_err(|e| format!("{n}: {e}"))? {
            let pts = bodies::us(&sub, 0.5);
            let area = signed_area(&pts).abs();
            if pts.len() < 3 || area.is_nan() || area < 0.25 {
                continue;
            }
            let body: Vec<V2> = pts.iter().map(|&p| base.front.to_body(p)).collect();
            loops.push(densify(&body, 0.03));
        }
        if loops.is_empty() {
            return Err(format!("{n} encloses no area"));
        }
        let off = loops
            .iter()
            .map(|l: &Vec<V2>| l.iter().map(|&p| signed_distance(&poly, p) < -OFF_BODY).collect())
            .collect();
        decals.push(Decal {
            off,
            over,
            loops,
            fill,
            opacity,
        });
    }
    Ok(decals)
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
/// `"e"` is the eye colour, or an object placing and sizing the eyes
/// (`c`, `at`, `gap`, `s`, `rim`, `rw`). `"p"` adds up to 6 extra parts
/// (`{"o","c","l","z"}`): then every outline shares one frame fitted to the
/// whole figure, each part is its own loft at its place in it, and the
/// parts draw in depth order (see [`part_order`]); the top-level outline
/// is the main part the eyes sit on. `"s"` adds up to 10 solids in head
/// space (see [`solids`]), in the same frame, which draw around everything
/// else.
///
/// The same recipe always returns the same name (`rich:<hash>`). Errors
/// are short human-readable messages: bad JSON, a missing or bad colour,
/// an unusable outline or layer path, more than 8 layers, a bad part or
/// more than 6.
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
    // with parts, every outline shares one frame; without, the main part
    // is the plain custom body of its outline
    let present = |k: &str| v.get(k).filter(|x| !x.is_null());
    // an empty list of solids is no solids
    let solid_list = present("s").filter(|s| s.as_array().is_none_or(|a| !a.is_empty()));
    let (base, parts_spec, scene) = match (present("p"), solid_list) {
        (None, None) => {
            let base_name = custom_body(outline)?;
            (find(base_name).ok_or("the outline didn't register")?, None, None)
        }
        (p, s) => {
            let (base, parts, layout, scene) = multi_part(outline, p, s, fnv(key))?;
            (base, Some((parts, layout)), scene)
        }
    };
    let shell = colour(v.get("c"), "shell")?;
    let (eyes, eye_box, rim, shut) = match v.get("e") {
        Some(e @ serde_json::Value::Object(_)) => {
            let eyes = colour(e.get("c"), "eye")?;
            let at = match e.get("at") {
                None | Some(serde_json::Value::Null) => None,
                Some(a) => {
                    let xy: Vec<f64> = a
                        .as_array()
                        .map(|a| a.iter().filter_map(|v| v.as_f64()).collect())
                        .unwrap_or_default();
                    if xy.len() != 2 || !xy.iter().all(|v| v.is_finite()) {
                        return Err("eye \"at\" must be [x, y]".to_owned());
                    }
                    Some([xy[0], xy[1]])
                }
            };
            let gap = eye_number(e, "gap")?;
            let scale = eye_number(e, "s")?;
            let eye_box = (at.is_some() || gap.is_some() || scale.is_some())
                .then(|| recipe_eyes(&base, at, gap, scale));
            let rim = match e.get("rim") {
                None | Some(serde_json::Value::Null) => None,
                r => {
                    let fill = colour(r, "eye rim")?;
                    let width = eye_box.as_ref().map_or(base.eyes.width, |(b, _)| b.width);
                    let rw = eye_number(e, "rw")?.unwrap_or(0.25).clamp(0.0, 1.2);
                    Some((fill, rw * width))
                }
            };
            let shut = match e.get("shut") {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(s)) if s == "l" => Some(0),
                Some(serde_json::Value::String(s)) if s == "r" => Some(1),
                Some(_) => return Err("eye \"shut\" must be \"l\" or \"r\"".to_owned()),
            };
            (eyes, eye_box, rim, shut)
        }
        e => (colour(e, "eye")?, None, None, None),
    };
    let decals = parse_layers(v.get("l"), &base, "")?;
    // an eye whose oval sits under the main part's over-layers is hidden
    let hidden = {
        let (eye, x) = eye_box.as_ref().map_or((&base.eyes, base.eye_x), |(b, x)| (b, *x));
        let y = -eye.shift_y.sin();
        let over: Vec<&Vec<V2>> = decals.iter().filter(|d| d.over).flat_map(|d| &d.loops).collect();
        let covered = |p: V2| over.iter().any(|l| inside(l, p));
        [-1.0, 1.0].map(|side| {
            let c = [x + side * eye.gap / 2.0, y];
            let (w, h) = (eye.width / 2.0, eye.height / 2.0);
            !over.is_empty()
                && [c, [c[0] - w, c[1]], [c[0] + w, c[1]], [c[0], c[1] - h], [c[0], c[1] + h]]
                    .iter()
                    .all(|&p| covered(p))
        })
    };
    let (parts, layout, overlap) = match parts_spec {
        Some((parts, layout)) => {
            let polys: Vec<Vec<V2>> = std::iter::once(&base)
                .chain(parts.iter().map(|p| &p.geometry))
                .map(|b| body_polygon(&b.outline.path))
                .collect();
            let overlap = polys
                .iter()
                .map(|a| polys.iter().map(|b| overlaps(a, b)).collect())
                .collect();
            (parts, layout, overlap)
        }
        None => (vec![], vec![], vec![]),
    };
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
                eye_box,
                rim,
                shut,
                hidden,
                parts,
                layout,
                overlap,
                solids: scene,
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
                shade: 0.0,
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

    /// Plain custom bodies draw and rest byte for byte as they did when
    /// figures began to fill the frame ([`fit_size`]).
    #[test]
    fn plain_custom_bodies_match_their_snapshot() {
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
        let want: [u64; 2] = [0x3674b535ad3db32b, 0xb4b2c0d29848d22d];
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

    /// `m` then `t`, as `[a, b, c, d, e, f]`.
    fn then(m: [f64; 6], t: [f64; 6]) -> [f64; 6] {
        [
            m[0] * t[0] + m[2] * t[1],
            m[1] * t[0] + m[3] * t[1],
            m[0] * t[2] + m[2] * t[3],
            m[1] * t[2] + m[3] * t[3],
            m[0] * t[4] + m[2] * t[5] + m[4],
            m[1] * t[4] + m[3] * t[5] + m[5],
        ]
    }

    /// An SVG `transform` list on `m`.
    fn transformed(mut m: [f64; 6], list: &str) -> [f64; 6] {
        for part in list.split(')').filter(|p| !p.trim().is_empty()) {
            let (name, args) = part.trim().split_once('(').expect("a transform");
            let v: Vec<f64> = args.split([' ', ',']).filter_map(|n| n.parse().ok()).collect();
            let t = match (name, v.as_slice()) {
                ("matrix", [a, b, c, d, e, f]) => [*a, *b, *c, *d, *e, *f],
                ("translate", [x]) => [1.0, 0.0, 0.0, 1.0, *x, 0.0],
                ("translate", [x, y]) => [1.0, 0.0, 0.0, 1.0, *x, *y],
                ("scale", [k]) => [*k, 0.0, 0.0, *k, 0.0, 0.0],
                ("scale", [x, y]) => [*x, 0.0, 0.0, *y, 0.0, 0.0],
                ("rotate", [a]) => {
                    let (s, c) = a.to_radians().sin_cos();
                    [c, s, -s, c, 0.0, 0.0]
                }
                ("rotate", [a, x, y]) => {
                    let (s, c) = a.to_radians().sin_cos();
                    let turn = then([1.0, 0.0, 0.0, 1.0, *x, *y], [c, s, -s, c, 0.0, 0.0]);
                    then(turn, [1.0, 0.0, 0.0, 1.0, -x, -y])
                }
                _ => panic!("a transform this can't read: {part}"),
            };
            m = then(m, t);
        }
        m
    }

    /// The furthest any path of `tree` goes, in the root's units, past its middle.
    fn furthest(tree: &MarkTree, mid: f64, outer: [f64; 6], most: &mut f64) {
        // what's in `defs` draws only through a `use` or a clip
        if tree.tag == "defs" {
            return;
        }
        let m = tree.attr("transform").map_or(outer, |t| transformed(outer, t));
        if let Some(d) = tree.attr("d") {
            let v: Vec<f64> = d.replace(|c: char| c.is_ascii_alphabetic(), " ").split_whitespace().filter_map(|n| n.parse().ok()).collect();
            for p in v.as_chunks::<2>().0 {
                let (x, y) = (m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]);
                *most = most.max((x - mid).abs()).max((y - mid).abs());
            }
        }
        for c in &tree.children {
            furthest(c, mid, m, most);
        }
    }

    fn spike(tip: [f64; 3]) -> String {
        let s = serde_json::json!([{"kind": "tube", "color": "#112233", "path": [[50.0, 50.0, 0.0, 3.0], [tip[0], tip[1], tip[2], 1.0]]}]);
        serde_json::json!({"o": "M50 20 C70 20 80 35 80 50 C80 65 70 80 50 80 C30 80 20 65 20 50 C20 35 30 20 50 20 Z", "c": "#FFFFFF",
            "l": [["M47 61 L53 61 L50 64 Z", "#112233"]], "e": "#112233", "s": s}).to_string()
    }

    /// However a figure turns and whatever state it's in, nothing leaves the avatar's canvas. These
    /// are the worst figures: a spike out to the top, the side, a corner, and toward the viewer,
    /// each sized by [`fit_size`], one of them right at [`FIT_REACH`].
    #[test]
    fn nothing_leaves_the_canvas() {
        let tab = shared_tables();
        let display = 52.48;
        let extent = (display * (0.92 / 0.6) * 100.0_f64).round() / 100.0;
        // `bot_blob`: a canvas half as big again as its cell, the mark 0.82 of the cell
        let half = display / 0.82 * 1.5 / 2.0;
        let reach = |tree: &MarkTree| {
            let mut most = 0.0;
            furthest(tree, extent / 2.0, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], &mut most);
            most
        };
        // `writing` turns any avatar into the kit's pencil, which is the same size on all of them
        let writing = |what: &str| what.starts_with("writing");
        let pencil = rich_run("blob", extent, display).iter().filter(|r| writing(&r.0)).map(|r| reach(&r.1)).fold(0.0, f64::max);
        for tip in [[50.0, -60.0, 0.0], [120.0, -20.0, 0.0], [160.0, 50.0, 0.0], [50.0, 50.0, 95.0], [100.0, 0.0, 70.0]] {
            let body = rich_body(&spike(tip)).unwrap();
            for (what, tree, ..) in rich_run(body, extent, display) {
                let most = if writing(&what) { pencil } else { half };
                assert!(reach(&tree) <= most, "{tip:?} {what}: {} past {most}", reach(&tree));
            }
            let mark = Mark::new(&tab, Some(body), 1.0, false, false, 7);
            let opts = PaintOpts { cutout: true, ink_paint: Some("#1884FC".to_owned()), ..Default::default() };
            let size = UhSize { display_size: Some(display), extent };
            for pitch in (-90..=90).step_by(15) {
                for yaw in (0..360).step_by(15) {
                    for roll in [-30.0, 0.0, 30.0] {
                        let mut frame = mark.frame();
                        frame.turn = [pitch as f64, yaw as f64, roll];
                        let got = reach(&uh(&frame, &opts, extent, "m", &size, &tab));
                        assert!(got <= half, "{tip:?} at {pitch} {yaw} {roll}: {got} past {half}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_figure_fills_the_frame_up_to_its_limits() {
        let figure = |w: f64, h: f64| [[-w / 2.0, -h / 2.0, 0.0], [w / 2.0, h / 2.0, 0.0]];
        let near = |a: f64, b: f64| assert!((a - b).abs() < 1e-9, "{a} against {b}");
        // what decides it is how tall it stands
        near(fit_size(figure(1.0, 2.0).into_iter()), FIT_HEIGHT / 2.0);
        // too wide for that, it's its width
        near(fit_size(figure(2.0, 1.0).into_iter()), FIT_WIDTH / 2.0);
        // a point far out decides it, however small the rest
        let far = [[1.5, 0.0, 0.0], [-0.5, -1.0, 0.0], [-0.5, 1.0, 0.0]];
        near(fit_size(far.into_iter()), FIT_REACH / 1.5);
        // one reaching toward the viewer counts too
        let deep = [[0.0, -1.0, 0.0], [0.0, 1.0, 1.4]];
        near(fit_size(deep.into_iter()), FIT_REACH / 1.0f64.hypot(1.4));
        // and a speck stops at the largest size
        near(fit_size(figure(0.1, 0.1).into_iter()), MAX_SIZE);
        // the frame beats everything: no least size holds a figure past it
        near(fit_size([[0.0, -1.0, 0.0], [0.0, 1.0, 6.0]].into_iter()), FIT_REACH / 1.0f64.hypot(6.0));
    }

    /// However the box of a recipe is filled, a figure stands as tall as the next.
    #[test]
    fn plain_outlines_of_any_shape_stand_as_tall() {
        let size = |o: &str| {
            let name = custom_body(o).unwrap();
            BODIES.with(|b| b.borrow()[name].size)
        };
        let tall = size("M30 10 L70 10 L70 90 L30 90 Z");
        let diamond = size("M50 10 L90 50 L50 90 L10 50 Z");
        assert!((tall - diamond).abs() < 0.02, "{tall} against {diamond}");
        // a square's corners swing out as it turns: the frame holds it back
        assert!(size("M10 10 L90 10 L90 90 L10 90 Z") < tall);
        let wide = size("M5 35 L95 35 L95 65 L5 65 Z");
        assert!(wide < tall + (FIT_WIDTH - FIT_HEIGHT), "a wide one stops at its width: {wide}");
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

    /// (a) a violet brute: spiky hair in the outline, a dark fringe, a cream
    /// face patch, faint blush, gold eyes.
    const BRUTE: &str = r##"{"o":"M50 8 C70 8 84 22 86 44 C90 62 84 82 70 90 C60 96 40 96 30 90 C16 82 10 62 14 44 C16 22 30 8 50 8 Z","c":"#7A4FB5","l":[["M16 38 C24 18 76 18 84 38 C70 30 30 30 16 38 Z","#3A2A5C",1],["M20 58 C20 42 36 38 50 38 C64 38 80 42 80 58 C80 78 66 90 50 90 C34 90 20 78 20 58 Z","#F6EBD6",1],["M26 70 L34 70 L34 75 L26 75 Z M66 70 L74 70 L74 75 L66 75 Z","#F2B8A8",0.55]],"e":"#FFC83D"}"##;
    /// (b) a teal masked hero: a gold band across the eyes, dark eyes.
    const HERO: &str = r##"{"o":"M50 6 C72 6 88 24 88 50 C88 76 72 94 50 94 C28 94 12 76 12 50 C12 24 28 6 50 6 Z","c":"#1E8E8A","l":[["M24 42 L76 42 L76 56 L24 56 Z","#F2C14E",1]],"e":"#16323A"}"##;
    /// (c) an orange cat: a cream muzzle on the lower face, dark eyes.
    const ORANGE_CAT: &str = r##"{"o":"M20 92 C8 80 8 55 14 40 L10 6 L38 26 C46 23 54 23 62 26 L90 6 L86 40 C92 55 92 80 80 92 C64 100 36 100 20 92 Z","c":"#F28C28","l":[["M31 79 C31 69 42 66 50 70 C58 66 69 69 69 79 C69 88 59 92 50 90 C41 92 31 88 31 79 Z","#FBE9CF",1]],"e":"#2A1E14"}"##;

    fn render_ink(mark: &Mark, extent: f64, display: f64, ink: &str) -> MarkTree {
        render_shaded(mark, extent, display, ink, 0.0)
    }

    fn render_shaded(mark: &Mark, extent: f64, display: f64, ink: &str, shade: f64) -> MarkTree {
        let tab = shared_tables();
        uh(
            &mark.frame(),
            &PaintOpts {
                cutout: true,
                ink_paint: Some(ink.to_owned()),
                shade,
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
        let a = rich_body(BRUTE).unwrap();
        assert!(a.starts_with("rich:"));
        assert_eq!(rich_body(BRUTE).unwrap(), a);
        assert_eq!(rich_body(&format!(" {BRUTE}\n")).unwrap(), a);
        assert_ne!(rich_body(HERO).unwrap(), a);
        assert_eq!(yn(a), a);
        // the geometry is the plain custom body's
        let recipe: serde_json::Value = serde_json::from_str(BRUTE).unwrap();
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

    /// The ink changes nothing on a rich body but its status morphs, and
    /// the colour picker doesn't reach those (`avatar::accent_of`).
    #[test]
    fn rich_ignores_the_ink() {
        let tab = shared_tables();
        let name = rich_body(BRUTE).unwrap();
        let mut a = 0xcbf29ce484222325u64;
        let mut b = a;
        let mut mark = Mark::new(&tab, Some(name), 1.0, false, false, 2);
        let mut dir = Director::new(&mut mark, "idle", 2, false, false, None);
        let mut drawn = 0;
        for (state, _) in super::super::clips::STATES {
            dir.set_state_pub(&mut mark, state);
            for f in 0..90 {
                step(&mut dir, &mut mark);
                if f % 5 == 0 && !frame_morph(&mark) {
                    drawn += 1;
                    hash_tree(&render_ink(&mark, 120.71, 78.72, "#1884FC"), &mut a);
                    hash_tree(&render_ink(&mark, 120.71, 78.72, "#FC2C3E"), &mut b);
                }
            }
        }
        assert!(drawn > 100, "only {drawn} frames without a morph");
        assert_eq!(a, b);
        dir.set_state_pub(&mut mark, "idle");
        for _ in 0..90 {
            step(&mut dir, &mut mark);
        }
        let mut ps = Vec::new();
        let tree = render_ink(&mark, 120.71, 78.72, "#1884FC");
        all_paths(&tree, &mut ps);
        assert!(ps.iter().all(|p| p.attr("fill") != Some("#1884FC")));
    }

    /// Hover darkening reaches every colour of a rich body, in the black
    /// ink of a light theme too, where the ink itself can't get darker.
    #[test]
    fn rich_darkens_with_the_edit_shade() {
        let tab = shared_tables();
        let name = rich_body(BRUTE).unwrap();
        let mark = Mark::new(&tab, Some(name), 1.0, false, false, 2);
        let plain = render_ink(&mark, 120.71, 78.72, "#000000");
        let dark = render_shaded(&mark, 120.71, 78.72, "#000000", 0.35);
        let fills = |t: &MarkTree| {
            let mut ps = Vec::new();
            all_paths(t, &mut ps);
            ps.iter().filter_map(|p| p.attr("fill").map(str::to_owned)).collect::<Vec<_>>()
        };
        let (pf, df) = (fills(&plain), fills(&dark));
        assert_eq!(pf.len(), df.len());
        assert!(pf.contains(&"#7A4FB5".to_owned()));
        for (p, d) in pf.iter().zip(&df) {
            assert_eq!(*d, super::super::darken(p, 0.35), "{p}");
        }
    }

    /// A rich avatar never rests on baked frames: those are one flat ink,
    /// and its ink is the primary one, so it would rest as a white shape.
    #[test]
    fn a_rich_shape_has_no_baked_rest() {
        let name = rich_body(HERO).unwrap();
        assert!(super::rich(name).is_some());
        assert!(rest_bytes(&format!("rich:{HERO}")).is_none());
    }

    /// A status morph leaves a rich avatar's colours: once it has settled,
    /// the body and the glyphs are the ink, and no recipe colour shows.
    #[test]
    fn a_settled_morph_is_all_ink() {
        const INK: &str = "#123456";
        let tab = shared_tables();
        let name = rich_body(HERO).unwrap();
        let mut mark = Mark::new(&tab, Some(name), 1.0, false, false, 2);
        let mut dir = Director::new(&mut mark, "idle", 2, false, false, None);
        let mut settled = 0;
        for (state, _) in super::super::clips::STATES {
            dir.set_state_pub(&mut mark, state);
            for _ in 0..75 {
                step(&mut dir, &mut mark);
            }
            if frame_eyes(&mark) {
                continue;
            }
            settled += 1;
            let tree = render_ink(&mark, 120.71, 78.72, INK);
            let mut ps = Vec::new();
            all_paths(&tree, &mut ps);
            assert!(ps.iter().any(|p| p.attr("fill") == Some(INK)), "{state}: no ink");
            for p in ps {
                for key in ["fill", "stroke"] {
                    let paint = p.attr(key).unwrap_or("none");
                    assert!(paint == "none" || paint == INK, "{state}: {key} {paint}");
                }
            }
        }
        assert!(settled > 10, "only {settled} states settled into a morph");
    }

    /// A rich body keeps its colours for as long as it has a face, then
    /// turns to the ink in one frame, never a blend of the two. A head turned
    /// away has no eyes to draw but keeps its face, so a spin doesn't flash.
    #[test]
    fn a_rich_body_turns_to_the_ink_as_its_face_goes() {
        const INK: &str = "#123456";
        let shell = serde_json::from_str::<serde_json::Value>(HERO).unwrap()["c"].as_str().unwrap().to_owned();
        let tab = shared_tables();
        let name = rich_body(HERO).unwrap();
        let mut mark = Mark::new(&tab, Some(name), 1.0, false, false, 2);
        let mut dir = Director::new(&mut mark, "idle", 2, false, false, None);
        let painted = |mark: &Mark, c: &str| {
            let tree = render_ink(mark, 120.71, 78.72, INK);
            let mut ps = Vec::new();
            all_paths(&tree, &mut ps);
            ps.iter().any(|p| p.attr("fill") == Some(c))
        };
        dir.set_state_pub(&mut mark, "thinking");
        let (mut faced, mut gone) = (0, 0);
        for _ in 0..45 {
            step(&mut dir, &mut mark);
            let face = frame_eyes(&mark);
            assert_eq!(painted(&mark, &shell), face, "shell, face {face}");
            assert_eq!(painted(&mark, INK), !face, "ink, face {face}");
            if face { faced += 1 } else { gone += 1 }
        }
        assert!(faced > 0 && gone > 0, "{faced} frames with a face, {gone} without");
        dir.set_state_pub(&mut mark, "idle");
        for _ in 0..45 {
            step(&mut dir, &mut mark);
        }
        mark.poke("spin", 1.0);
        for _ in 0..45 {
            step(&mut dir, &mut mark);
            assert!(!painted(&mark, INK), "a spin showed the ink");
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
        for gy in -2..=2 {
            for gx in -2..=2 {
                mark.set_gaze(Some([gx as f64 / 2.0, gy as f64 / 2.0]));
                for f in 0..10 {
                    mark.advance(1.0 / 30.0);
                    let shade = if (gx + gy) % 2 == 0 { 0.0 } else { 0.7 };
                    out.push((format!("rest gaze {gx},{gy} f{f}"), render_shaded(&mark, extent, display, "#1884FC", shade), frame_eyes(&mark), frame_morph(&mark)));
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
        for (recipe, patch, eye) in [(BRUTE, 1usize, "#FFC83D"), (ORANGE_CAT, 0, "#2A1E14")] {
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
        for recipe in [BRUTE, HERO, ORANGE_CAT] {
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
        for (label, body) in [("plain", custom_body(CAT).unwrap()), ("rich", rich_body(ORANGE_CAT).unwrap()), ("rich brute", rich_body(BRUTE).unwrap()), ("bucket hat", rich_body(BUCKET_HAT).unwrap()), ("horn hat", rich_body(HORN_HAT).unwrap())] {
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
            ("brute", rich_body(BRUTE).unwrap()),
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

    // -- recipe eyes (round 4) -------------------------------------------------

    /// Big eyes: a plain green head, no lens layers, big pale eyes with dark rims.
    const BIG_EYES: &str = r##"{"o":"M50 8 C70 8 86 28 86 52 C86 76 70 92 50 92 C30 92 14 76 14 52 C14 28 30 8 50 8 Z","c":"#3F8F5F","e":{"c":"#FFF6D8","s":1.9,"rim":"#24402F","rw":0.35,"at":[50,48],"gap":30}}"##;
    /// Goggles: a brown head, a skin face patch, dark eyes in gold rims.
    const GOGGLES: &str = r##"{"o":"M50 5 C77 5 92 26 92 52 C92 79 74 96 50 96 C26 96 8 79 8 52 C8 26 23 5 50 5 Z","c":"#6B4226","l":[["M20 60 C20 43 34 35 50 35 C66 35 80 43 80 60 C80 81 66 92 50 92 C34 92 20 81 20 60 Z","#F1C7A0",1]],"e":{"c":"#2A1E14","at":[50,54],"gap":28,"s":1.25,"rim":"#D4A017","rw":0.55}}"##;
    /// A ninja: a dark hood with a skin eye band at y 46..58, the eyes in it.
    const NINJA: &str = r##"{"o":"M50 7 C71 7 87 26 87 51 C87 76 71 93 50 93 C29 93 13 76 13 51 C13 26 29 7 50 7 Z","c":"#1E2A3A","l":[["M9 45 L91 45 L91 59 L9 59 Z","#E9C49A",1]],"e":{"c":"#111111","at":[50,52],"gap":24,"s":0.9}}"##;

    /// String-`"e"` recipes keep round 3's eyes (no recipe eye box, no
    /// rim) and draw byte for byte as this snapshot. It moved once, in
    /// round 4, from round 3's (0xf3dbe89e8cc1a6cc, 0x333f3d3298421459,
    /// 0x8dbe3467ae0e694c): where a decal crosses the limb, the cut now
    /// follows the visible edge instead of a straight chord (the hero, with
    /// no layer at the limb, didn't move). It moved again when status morphs
    /// left the recipe's colours for the ink; the shade is also exact now,
    /// not read back from a rounded ink (0.7, it used to read 0.6984).
    /// It moved again when the brute and the hero were redrawn as original
    /// avatars, palette and geometry both, and the cat kept its value then.
    /// All three moved when figures began to fill the frame ([`fit_size`]).
    #[test]
    fn rich_string_eyes_match_their_snapshot() {
        for recipe in [BRUTE, HERO, ORANGE_CAT] {
            let r = rich(rich_body(recipe).unwrap()).unwrap();
            assert!(r.eye_box.is_none() && r.rim.is_none());
        }
        let mut got = Vec::new();
        for recipe in [BRUTE, HERO, ORANGE_CAT] {
            let name = rich_body(recipe).unwrap();
            let mut h = body_hash(name, "hex");
            for (_, tree, _, _) in rich_run(name, 80.47, 52.48) {
                hash_tree(&tree, &mut h);
            }
            got.push(h);
        }
        assert_eq!(got, vec![0x46dcdb07f206087c, 0x37fe198591baa045, 0x3c4a15bc7ced765b]);
    }

    /// The two eye centroids at the front pose in a 255px sprite.
    fn front_eyes(name: &'static str) -> Vec<Vec<V2>> {
        let eyes = super::super::Face::new().eyes();
        let rot = bodies::Rot { yaw: 0.0, pitch: 0.0, roll: 0.0 };
        bodies::dollar_h(name, &rot, 255.0, Some(&eyes))
            .eyes
            .iter()
            .flat_map(|d| loops(d))
            .collect()
    }

    fn centroid(l: &[V2]) -> V2 {
        let n = l.len() as f64;
        let (x, y) = l.iter().fold((0.0, 0.0), |(x, y), p| (x + p[0], y + p[1]));
        [x / n, y / n]
    }

    /// The recipe's `at` and `gap` land where asked at the front pose.
    #[test]
    fn recipe_eyes_land_at_at() {
        for (recipe, at, gap) in [(BIG_EYES, [50.0, 48.0], 30.0), (NINJA, [50.0, 52.0], 24.0), (GOGGLES, [50.0, 54.0], 28.0)] {
            let name = rich_body(recipe).unwrap();
            let b = find(name).unwrap();
            let r = b.size * CS * 255.0 / US;
            let want = b.front.to_body(at);
            let want = [127.5 + r * want[0], 127.5 - r * want[1]];
            let eyes = front_eyes(name);
            assert_eq!(eyes.len(), 2, "{name}");
            let (a, c) = (centroid(&eyes[0]), centroid(&eyes[1]));
            let mid = [(a[0] + c[0]) / 2.0, (a[1] + c[1]) / 2.0];
            let got_gap = (a[0] - c[0]).hypot(a[1] - c[1]);
            let want_gap = gap * b.front.scale * b.front.renorm[0] / X * r;
            println!(
                "{name}: centre {mid:.1?} want {want:.1?}, gap {got_gap:.1} want {want_gap:.1} (of 255)"
            );
            assert!((mid[0] - want[0]).hypot(mid[1] - want[1]) <= 2.0, "{name} centre");
            assert!((got_gap - want_gap).abs() <= want_gap * 0.03, "{name} gap");
        }
    }

    /// Eyes asked for past the outline are fitted inside it: shrunk, then
    /// pulled toward the face.
    #[test]
    fn recipe_eyes_are_clamped_inside() {
        let recipe = r##"{"o":"M50 8 C70 8 86 28 86 52 C86 76 70 92 50 92 C30 92 14 76 14 52 C14 28 30 8 50 8 Z","c":"#3F8F5F","e":{"c":"#FFFFFF","s":2.2,"at":[86,30],"gap":40}}"##;
        let name = rich_body(recipe).unwrap();
        let rich = rich(name).unwrap();
        let (eye, _) = rich.eye_box.as_ref().unwrap();
        let b = find(name).unwrap();
        let asked = SQUARE_EYE[0] * SQUARE_SIZE / b.size * 2.2;
        println!("asked width {asked:.3}, got {:.3}", eye.width);
        assert!(eye.width < asked);
        let rot = bodies::Rot { yaw: 0.0, pitch: 0.0, roll: 0.0 };
        let outline = loops(&bodies::im(name, &rot, 255.0)[0]).remove(0);
        for p in front_eyes(name).iter().flatten() {
            assert!(signed_distance(&outline, *p) > 0.0, "eye point {p:?} outside");
        }
    }

    /// A rim rings each eye in every frame the eyes are drawn and is gone
    /// when they are; it sits over the layers and under the eye.
    #[test]
    fn rims_follow_the_eyes() {
        for recipe in [BIG_EYES, GOGGLES] {
            let name = rich_body(recipe).unwrap();
            for (extent, display) in [(80.47, 52.48), (120.71, 78.72)] {
                for (label, tree, has_eyes, _) in rich_run(name, extent, display) {
                    let mut ps = Vec::new();
                    all_paths(&tree, &mut ps);
                    let eyes = ps.iter().filter(|p| p.attr("data-eye").is_some()).count();
                    let rims = ps.iter().filter(|p| p.attr("data-rim").is_some()).count();
                    if eyes > 0 {
                        assert!(rims > 0, "{name} at {display}: {label}: eyes without rims");
                        let last_rim = ps.iter().rposition(|p| p.attr("data-rim").is_some()).unwrap();
                        let first_eye = ps.iter().position(|p| p.attr("data-eye").is_some()).unwrap();
                        let last_layer = ps.iter().rposition(|p| p.attr("data-layer").is_some());
                        assert!(last_rim < first_eye && last_layer.is_none_or(|l| l < last_rim));
                    }
                    if !has_eyes {
                        assert_eq!(rims, 0, "{name} at {display}: {label}: rims without eyes");
                    }
                }
            }
        }
    }

    /// Height (screen units) of the taller eye of a tree.
    fn eye_height(tree: &MarkTree) -> f64 {
        let mut ps = Vec::new();
        all_paths(tree, &mut ps);
        ps.iter()
            .filter(|p| p.attr("data-eye").is_some())
            .flat_map(|p| loops(p.attr("d").unwrap_or("")))
            .map(|l| {
                let ys = l.iter().map(|p| p[1]);
                let (lo, hi) = ys.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), y| (lo.min(y), hi.max(y)));
                hi - lo
            })
            .fold(0.0, f64::max)
    }

    /// The frame of an idle run with the lids most closed.
    fn blink_mark(name: &'static str) -> Mark {
        let tab = shared_tables();
        let mut mark = Mark::new(&tab, Some(name), 1.0, false, false, 5);
        let mut dir = Director::new(&mut mark, "idle", 5, false, false, None);
        let mut best = (0.0, 0);
        for f in 0..600 {
            step(&mut dir, &mut mark);
            if mark.fe() > best.0 {
                best = (mark.fe(), f);
            }
        }
        let mut mark = Mark::new(&tab, Some(name), 1.0, false, false, 5);
        let mut dir = Director::new(&mut mark, "idle", 5, false, false, None);
        for _ in 0..=best.1 {
            step(&mut dir, &mut mark);
        }
        mark
    }

    /// Big eyes still blink: at the closed-lid frame the eye is a slit.
    #[test]
    fn big_eyes_blink() {
        let tab = shared_tables();
        let name = rich_body(BIG_EYES).unwrap();
        let open = eye_height(&render_ink(&Mark::new(&tab, Some(name), 1.0, false, false, 5), 144.0, 96.0, "#1884FC"));
        let blink = blink_mark(name);
        let shut = eye_height(&render_ink(&blink, 144.0, 96.0, "#1884FC"));
        println!("lids {:.2}: eye height {shut:.1} vs open {open:.1}", blink.fe());
        assert!(blink.fe() > 0.8);
        assert!(shut < open * 0.4, "{shut} vs {open}");
    }

    /// Writes the round-4 sheets as SVG into `MARK_EYES_DIR`.
    #[test]
    fn render_eye_sheets() {
        let Ok(dir) = std::env::var("MARK_EYES_DIR") else {
            return;
        };
        let tab = shared_tables();
        let rows: Vec<(&str, &'static str)> = vec![
            ("blob", "sphere"),
            ("big eyes", rich_body(BIG_EYES).unwrap()),
            ("goggles", rich_body(GOGGLES).unwrap()),
            ("ninja", rich_body(NINJA).unwrap()),
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
        let mut svg = head(110.0 + 4.0 * 150.0, rows.len() as f64 * 150.0 + 30.0);
        for (c, title) in ["rest 64pt", "rest 80pt", "rest 96pt", "blink 96pt"].iter().enumerate() {
            svg += &label(130.0 + c as f64 * 150.0, 18.0, title);
        }
        for (r, (name, body)) in rows.iter().enumerate() {
            let y = 30.0 + r as f64 * 150.0;
            svg += &label(8.0, y + 70.0, name);
            for (c, pt) in [64.0, 80.0, 96.0, 96.0].iter().enumerate() {
                let extent = pt * 0.82 * 0.92 / 0.6;
                let (x0, y0) = (110.0 + c as f64 * 150.0 + (150.0 - extent) / 2.0, y + (150.0 - extent) / 2.0);
                if *body == "sphere" && c < 3 {
                    let bytes = super::super::paint::rest_bytes("blob").unwrap();
                    svg += &rest_svg(&bytes, extent, x0, y0, ink);
                    continue;
                }
                let mark = if c == 3 { blink_mark(body) } else { Mark::new(&tab, Some(body), 1.0, false, false, 1) };
                let mut t = render_ink(&mark, extent, pt * 0.82, ink);
                t.attrs.retain(|(k, _)| *k != "id");
                svg += &tree_svg(&t, x0, y0, 1.0);
            }
        }
        std::fs::write(format!("{dir}/rest.svg"), svg + "</svg>").unwrap();
        let poses: [(&str, V3); 5] = [
            ("yaw -40", [0.0, -40.0, 0.0]),
            ("front", [0.0, 0.0, 0.0]),
            ("yaw 40", [0.0, 40.0, 0.0]),
            ("yaw 72", [0.0, 72.0, 0.0]),
            ("pitch 40", [40.0, 20.0, 0.0]),
        ];
        let mut svg = head(110.0 + 5.0 * 150.0, rows.len() as f64 * 150.0 + 30.0);
        for (c, (title, _)) in poses.iter().enumerate() {
            svg += &label(140.0 + c as f64 * 150.0, 18.0, title);
        }
        for (r, (name, body)) in rows.iter().enumerate() {
            let y = 30.0 + r as f64 * 150.0;
            svg += &label(8.0, y + 70.0, name);
            for (c, (_, rot)) in poses.iter().enumerate() {
                svg += &pose_svg(body, *rot, 150.0, 100.0 + c as f64 * 150.0, y, &format!("q{r}x{c}"));
            }
        }
        std::fs::write(format!("{dir}/turn.svg"), svg + "</svg>").unwrap();
    }

    // -- multi-part rich bodies (round 5) --------------------------------------

    /// (a) A bucket hat: an auburn-haired head with a skin face (the main part) under
    /// a blue bucket hat with a white band.
    const BUCKET_HAT: &str = r##"{"o":"M24 64 C24 46 36 38 50 38 C64 38 76 46 76 64 C76 80 64 92 50 92 C36 92 24 80 24 64 Z","c":"#6B3A1E","l":[["M29 68 C29 56 39 51 50 51 C61 51 71 56 71 68 C71 82 61 90 50 90 C39 90 29 82 29 68 Z","#F2C9A0",1],["M39 74 L41 74 L41 76 L39 76 Z","#D98A6A",1]],"e":{"c":"#111111","at":[50,64],"gap":16,"s":0.9},"p":[{"o":"M12 46 L28 44 L34 16 L66 16 L72 44 L88 46 L88 52 L12 52 Z","c":"#4C8CC9","l":[["M31 34 L69 34 L69 40 L31 40 Z","#F4F4F4",1]],"z":0.1}]}"##;
    /// (b) A horned hat: a pale blue head with white-rimmed eyes and an orange nose,
    /// a green hat with a square white badge and a dark brim band, two grey horns.
    const HORN_HAT: &str = r##"{"o":"M26 70 C26 54 38 48 50 48 C62 48 74 54 74 70 C74 84 62 94 50 94 C38 94 26 84 26 70 Z","c":"#8FB8DE","l":[["M47 78 L53 78 L50 83 Z","#E8743B",1]],"e":{"c":"#111111","at":[50,69],"gap":18,"s":1.0,"rim":"#FFFFFF","rw":0.5},"p":[{"o":"M28 52 L30 46 L36 44 L38 22 L62 22 L64 44 L70 46 L72 52 C60 56 40 56 28 52 Z","c":"#6BBF8A","l":[["M45 30 L55 30 L55 38 L45 38 Z","#FFFFFF",1],["M29 47 L71 47 L71 52 L29 52 Z","#3E8E62",1]],"z":0.1},{"o":"M28 46 L16 18 L26 14 L38 44 Z","c":"#9AA0A6","z":-0.05},{"o":"M72 46 L84 18 L74 14 L62 44 Z","c":"#9AA0A6","z":-0.05}]}"##;
    /// (c) Cloud hair: a sky-blue cloud-hair head (main) with a skin face, and two
    /// small clouds floating off its upper left and right.
    const CLOUD_HAIR: &str = r##"{"o":"M28 42 C22 32 32 22 42 26 C46 14 60 14 64 24 C74 20 84 30 78 40 C88 46 86 60 78 62 C82 74 72 86 60 86 C54 94 42 92 38 86 C26 88 16 76 22 64 C12 58 16 46 28 42 Z","c":"#8EC5FF","l":[["M36 62 C36 52 43 48 50 48 C57 48 64 52 64 62 C64 74 57 82 50 82 C43 82 36 74 36 62 Z","#F2C9A0",1]],"e":{"c":"#2B3A67","at":[50,63],"gap":14,"s":0.8,"rim":"#FFFFFF","rw":0.3},"p":[{"o":"M6 24 C4 18 10 14 14 16 C16 10 24 10 26 16 C32 16 32 24 28 26 C26 30 10 30 6 24 Z","c":"#D6E9FF"},{"o":"M94 24 C96 18 90 14 86 16 C84 10 76 10 74 16 C68 16 68 24 72 26 C74 30 90 30 94 24 Z","c":"#D6E9FF"}]}"##;
    /// A head with a hair bun behind it (z -0.5), overlapping its top.
    const BUN: &str = r##"{"o":"M24 64 C24 46 36 38 50 38 C64 38 76 46 76 64 C76 80 64 92 50 92 C36 92 24 80 24 64 Z","c":"#F2C9A0","e":"#111111","p":[{"o":"M36 30 C36 18 42 10 50 10 C58 10 64 18 64 30 C64 40 58 44 50 44 C42 44 36 40 36 30 Z","c":"#3A2618","z":-0.5}]}"##;

    fn bbox_centre(l: &[V2]) -> V2 {
        let (mut x0, mut x1, mut y0, mut y1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
        for p in l {
            x0 = x0.min(p[0]);
            x1 = x1.max(p[0]);
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1]);
        }
        [(x0 + x1) / 2.0, (y0 + y1) / 2.0]
    }

    /// Seen from the front, every part sits where the recipe's box puts it
    /// relative to the head (one shared frame, not one per part).
    #[test]
    fn parts_keep_their_placement() {
        let front = bodies::Rot { yaw: 0.0, pitch: 0.0, roll: 0.0 };
        for recipe in [BUCKET_HAT, HORN_HAT, CLOUD_HAIR] {
            let name = rich_body(recipe).unwrap();
            let r = rich(name).unwrap();
            let main = find(name).unwrap();
            let v: serde_json::Value = serde_json::from_str(recipe).unwrap();
            let boxed = |o: &str| bbox_centre(&outline_points(o).unwrap());
            let main_box = boxed(v["o"].as_str().unwrap());
            let radius = bodies::sprite_radius(name, 255.0);
            let at = |b: &'static str| bbox_centre(&loops(&bodies::im(b, &front, 255.0)[0])[0]);
            let main_px = at(name);
            for (i, part) in r.parts.iter().enumerate() {
                let want = boxed(v["p"][i]["o"].as_str().unwrap());
                let k = main.front.scale / X * radius;
                let want = [(want[0] - main_box[0]) * k, (want[1] - main_box[1]) * k];
                let got = at(part.body);
                let got = [got[0] - main_px[0], got[1] - main_px[1]];
                let err = (got[0] - want[0]).hypot(got[1] - want[1]);
                println!("{name} part {}: offset {got:.1?} want {want:.1?} ({err:.2} of 255)", i + 1);
                assert!(err <= 1.5, "{name} part {}", i + 1);
            }
        }
    }

    /// A bun behind the head draws behind it from the front and in front
    /// of it once the head turns away; horns either side swap with the
    /// turn.
    #[test]
    fn parts_draw_in_depth_order() {
        let r = rich(rich_body(BUN).unwrap()).unwrap();
        let pos = |order: &[usize], i: usize| order.iter().position(|&o| o == i).unwrap();
        for (yaw, bun_first) in [(0.0, true), (33.0, true), (-60.0, true), (180.0, false), (150.0, false)] {
            let order = part_order(&r, &bodies::Rot { yaw, pitch: -16.0, roll: 0.0 });
            assert_eq!(pos(&order, 1) < pos(&order, 0), bun_first, "yaw {yaw}: {order:?}");
        }
        let r = rich(rich_body(HORN_HAT).unwrap()).unwrap();
        // parts: 1 hat, 2 left horn, 3 right horn
        let order = part_order(&r, &bodies::Rot { yaw: 60.0, pitch: 0.0, roll: 0.0 });
        assert!(pos(&order, 2) > pos(&order, 3), "yaw 60: {order:?}");
        let order = part_order(&r, &bodies::Rot { yaw: -60.0, pitch: 0.0, roll: 0.0 });
        assert!(pos(&order, 3) > pos(&order, 2), "yaw -60: {order:?}");
        // the hat stays over the head while the face is toward the viewer
        for yaw in [-70.0, -33.0, 0.0, 33.0, 70.0] {
            for pitch in [-30.0, -16.0, 0.0, 30.0] {
                let order = part_order(&r, &bodies::Rot { yaw, pitch, roll: 0.0 });
                assert!(pos(&order, 1) > pos(&order, 0), "yaw {yaw} pitch {pitch}: {order:?}");
            }
        }
    }

    /// Each part's layers are clipped to that part's own outline.
    #[test]
    fn part_layers_clip_to_their_part() {
        let tab = shared_tables();
        let name = rich_body(HORN_HAT).unwrap();
        let mark = Mark::new(&tab, Some(name), 1.0, false, false, 1);
        let tree = render_ink(&mark, 120.71, 78.72, "#1884FC");
        fn walk<'a>(t: &'a MarkTree, out: &mut Vec<&'a MarkTree>) {
            out.push(t);
            for c in &t.children {
                walk(c, out);
            }
        }
        let mut all = Vec::new();
        walk(&tree, &mut all);
        let clips: Vec<&str> = all.iter().filter(|t| t.tag == "clipPath").filter_map(|t| t.attr("id")).collect();
        for i in 1..=3 {
            assert!(clips.contains(&format!("m-p{i}").as_str()), "{clips:?}");
        }
        let groups: Vec<_> = all.iter().filter(|t| t.tag == "g" && t.attr("data-part").is_some()).collect();
        // the hat has layers; the horns none
        assert_eq!(groups.len(), 1);
        for g in groups {
            let i = g.attr("data-part").unwrap();
            assert_eq!(g.attr("clip-path"), Some(format!("url(#m-p{i})").as_str()));
            assert!(g.children.iter().all(|c| c.attr("data-layer").is_some()));
        }
        // the main part's own layers clip to the main outline
        let main = all.iter().find(|t| t.attr("clip-path") == Some("url(#m-clip)")).unwrap();
        assert!(main.children.iter().any(|c| c.attr("data-eye").is_some()));
    }

    /// The bucket hat at rest: the hat draws over the head and the eyes stay
    /// visible below its brim, at every avatar size and through a gaze
    /// sweep.
    #[test]
    fn eyes_show_under_a_hat() {
        let tab = shared_tables();
        let name = rich_body(BUCKET_HAT).unwrap();
        for (extent, display) in [(80.47, 52.48), (100.59, 65.6), (120.71, 78.72)] {
            let mut mark = Mark::new(&tab, Some(name), 1.0, false, false, 1);
            for g in 0..=20 {
                if g > 0 {
                    mark.set_gaze(Some([(g as f64 / 10.0 - 1.0), 0.4 - g as f64 / 25.0]));
                    for _ in 0..6 {
                        mark.advance(1.0 / 30.0);
                    }
                }
                let tree = render_ink(&mark, extent, display, "#1884FC");
                let mut ps = Vec::new();
                all_paths(&tree, &mut ps);
                let eye_at = ps.iter().position(|p| p.attr("data-eye").is_some()).expect("eyes");
                let hat_at = ps.iter().position(|p| p.attr("data-part") == Some("1")).expect("hat");
                assert!(hat_at > eye_at, "the hat draws over the head");
                let hat = loops(ps[hat_at].attr("d").unwrap()).remove(0);
                let eyes: Vec<V2> = ps
                    .iter()
                    .filter(|p| p.attr("data-eye").is_some())
                    .flat_map(|p| loops(p.attr("d").unwrap()))
                    .map(|l| centroid(&l))
                    .collect();
                assert_eq!(eyes.len(), 2, "at {display} gaze {g}");
                for e in eyes {
                    assert!(!inside(&hat, e), "at {display} gaze {g}: eye {e:?} under the hat");
                }
            }
        }
    }

    /// Through the avatar run (rest gaze sweep with the edit shade, every
    /// state clip, the pokes, status morphs) the extra parts show in every
    /// frame with eyes and go with the eyes; the main shell shows with them,
    /// and only a status morph, past the face, takes it to the ink.
    #[test]
    fn parts_follow_the_eyes() {
        let name = rich_body(BUCKET_HAT).unwrap();
        for (extent, display) in [(80.47, 52.48), (120.71, 78.72)] {
            for (label, tree, has_eyes, _) in rich_run(name, extent, display) {
                let mut ps = Vec::new();
                all_paths(&tree, &mut ps);
                let hat = ps.iter().any(|p| p.attr("data-part") == Some("1"));
                let shell = ps.iter().any(|p| p.attr("fill") == Some("#6B3A1E") || p.attr("fill").is_some_and(|f| f.starts_with("rgb(")));
                let ink = ps.iter().any(|p| p.attr("fill") == Some("#1884FC"));
                assert!(shell || (ink && !has_eyes), "{label}: no shell");
                assert_eq!(hat, has_eyes, "{label} at {display}: hat {hat}, eyes {has_eyes}");
            }
        }
    }

    #[test]
    fn rich_parts_errors() {
        let head = r##""o":"M24 64 C24 46 36 38 50 38 C64 38 76 46 76 64 C76 80 64 92 50 92 C36 92 24 80 24 64 Z","c":"#141414","e":"#111111""##;
        let part = r##"{"o":"M10 10 L30 10 L30 30 Z","c":"#FFFFFF"}"##;
        let seven = [part; 7].join(",");
        for p in [
            r#""p":{}"#.to_owned(),
            r#""p":[1]"#.to_owned(),
            r##""p":[{"c":"#FFFFFF"}]"##.to_owned(),
            r##""p":[{"o":"M0 0 L10 10","c":"#FFFFFF"}]"##.to_owned(),
            r##""p":[{"o":"M10 10 L30 10 L30 30 Z","c":"white"}]"##.to_owned(),
            r##""p":[{"o":"M10 10 L30 10 L30 30 Z","c":"#FFFFFF","z":"up"}]"##.to_owned(),
            r##""p":[{"o":"M10 10 L30 10 L30 30 Z","c":"#FFFFFF","l":[["M0 0 A1 1 0 0 0 5 5 Z","#000000"]]}]"##.to_owned(),
            format!(r#""p":[{seven}]"#),
        ] {
            let recipe = format!("{{{head},{p}}}");
            assert!(rich_body(&recipe).is_err(), "{p} should be rejected");
        }
        assert!(rich_body(&format!("{{{head},\"p\":[{part}]}}")).is_ok());
    }

    /// Writes the round-5 sheets as SVG into `MARK_PARTS_DIR`.
    #[test]
    fn render_part_sheets() {
        let Ok(dir) = std::env::var("MARK_PARTS_DIR") else {
            return;
        };
        part_sheets(
            &dir,
            "#2A2A2E",
            &[
                ("blob", "sphere"),
                ("bucket hat", rich_body(BUCKET_HAT).unwrap()),
                ("horn hat", rich_body(HORN_HAT).unwrap()),
                ("cloud hair", rich_body(CLOUD_HAIR).unwrap()),
            ],
        );
    }

    /// `rest.svg` (rest at 64/80/96pt) and `turn.svg` (yaw -40/0/40/72,
    /// pitch 40) for `rows` on a `bg` background.
    fn part_sheets(dir: &str, bg: &str, rows: &[(&str, &'static str)]) {
        let tab = shared_tables();
        let head = |w: f64, h: f64| {
            format!("<svg xmlns='http://www.w3.org/2000/svg' width='{w}' height='{h}'><rect width='100%' height='100%' fill='{bg}'/>")
        };
        let ink = "#1884FC";
        let mut svg = head(110.0 + 3.0 * 150.0, rows.len() as f64 * 150.0 + 30.0);
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
                    continue;
                }
                let mark = Mark::new(&tab, Some(body), 1.0, false, false, 1);
                let mut t = render_ink(&mark, extent, pt * 0.82, ink);
                t.attrs.retain(|(k, _)| *k != "id");
                let mut x = String::new();
                xml(&t, &mut x);
                // each cell's ids must be unique within the sheet
                let x = x.replace("m-", &format!("c{r}{c}-"));
                svg += &format!("<g transform='translate({x0} {y0})'>{x}</g>");
            }
        }
        std::fs::write(format!("{dir}/rest.svg"), svg + "</svg>").unwrap();
        let poses: [(&str, V3); 5] = [
            ("yaw -40", [0.0, -40.0, 0.0]),
            ("front", [0.0, 0.0, 0.0]),
            ("yaw 40", [0.0, 40.0, 0.0]),
            ("yaw 72", [0.0, 72.0, 0.0]),
            ("pitch 40", [40.0, 20.0, 0.0]),
        ];
        let mut svg = head(110.0 + 5.0 * 150.0, rows.len() as f64 * 150.0 + 30.0);
        for (c, (title, _)) in poses.iter().enumerate() {
            svg += &label(140.0 + c as f64 * 150.0, 18.0, title);
        }
        for (r, (name, body)) in rows.iter().enumerate() {
            let y = 30.0 + r as f64 * 150.0;
            svg += &label(8.0, y + 70.0, name);
            for (c, (_, rot)) in poses.iter().enumerate() {
                svg += &pose_svg(body, *rot, 150.0, 100.0 + c as f64 * 150.0, y, &format!("q{r}x{c}"));
            }
        }
        std::fs::write(format!("{dir}/turn.svg"), svg + "</svg>").unwrap();
    }

    const BUCKET_HAT3: &str = r##"{"o":"M25 61 L19 57 L25 51 C27 39 38 33 50 33 C62 33 73 39 75 51 L81 57 L75 61 C73 79 62 91 50 92 C38 91 27 79 25 61 Z","c":"#F6D3B0","l":[["M28 50 C36 56 44 54 50 50 C56 54 64 56 72 50 L72 46 L28 46 Z","#6B3A1E"],["M21 56 L25 53 L25 60 Z","#E8B98F"],["M79 56 L75 53 L75 60 Z","#E8B98F"],["M59 73 L61 73 L61 75 L59 75 Z","#D98A6A"],["M62 75 L64 75 L64 77 L62 77 Z","#D98A6A"],["M65 72 L67 72 L67 74 L65 74 Z","#D98A6A"]],"e":{"c":"#111111","at":[50,68],"gap":20,"s":0.85,"rim":"#FFFFFF","rw":0.6},"p":[{"o":"M24 72 L4 68 L14 54 L2 42 L17 38 L25 31 C35 26 65 26 75 31 L83 38 L98 42 L86 54 L96 68 L76 72 L72 64 C64 58 36 58 28 64 Z","c":"#6B3A1E","z":-0.1HAIRD},{"o":"M4 42 L26 38 L32 10 L68 10 L74 38 L96 42 L96 48 L4 48 Z","c":"#4C8CC9","z":0.1HATD,"l":[["M30 24 L70 24 L70 31 L30 31 Z","#F4F4F4"],["M6 44 L94 44 L94 47 L6 47 Z","#3E76AD"],["M38 14 L46 14 L44 20 L37 20 Z","#8DBBE6",0.8]]}]}"##;

    fn bucket_hat3(hair: Option<f64>, hat: Option<f64>) -> String {
        let d = |v: Option<f64>| v.map_or(String::new(), |v| format!(",\"d\":{v}"));
        BUCKET_HAT3.replace("HAIRD", &d(hair)).replace("HATD", &d(hat))
    }

    /// The visible area of part `part` in a rich tree (grid-sampled, screen
    /// units²), split left/right of the main part's centre: the part's
    /// outline minus every part drawn over it.
    fn visible_part(tree: &MarkTree, part: &str, shell: &str) -> (f64, f64) {
        let mut ps = Vec::new();
        all_paths(tree, &mut ps);
        // the drawn outlines in paint order: the main body and the parts
        let shapes: Vec<(Option<&str>, Vec<V2>)> = ps
            .iter()
            .filter(|p| {
                p.attr("data-layer").is_none()
                    && p.attr("data-eye").is_none()
                    && p.attr("data-rim").is_none()
                    && (p.attr("data-part").is_some() || p.attr("fill") == Some(shell))
            })
            .map(|p| (p.attr("data-part"), loops(p.attr("d").unwrap_or("")).remove(0)))
            .collect();
        let Some(at) = shapes.iter().position(|(id, _)| *id == Some(part)) else {
            return (0.0, 0.0);
        };
        let main = &shapes.iter().find(|(id, _)| id.is_none()).unwrap().1;
        let cx = centroid(main)[0];
        let target = &shapes[at].1;
        let (mut x0, mut x1, mut y0, mut y1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
        for p in target {
            x0 = x0.min(p[0]);
            x1 = x1.max(p[0]);
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1]);
        }
        let main_at = shapes.iter().position(|(id, _)| id.is_none()).unwrap();
        let above: Vec<&Vec<V2>> = shapes
            .iter()
            .enumerate()
            .filter(|(i, _)| *i > at || (*i == main_at && main_at > at))
            .map(|(_, s)| &s.1)
            .collect();
        const G: usize = 120;
        let cell = (x1 - x0) * (y1 - y0) / (G * G) as f64;
        let (mut left, mut right) = (0.0, 0.0);
        for i in 0..G {
            for j in 0..G {
                let p = [x0 + (x1 - x0) * (i as f64 + 0.5) / G as f64, y0 + (y1 - y0) * (j as f64 + 0.5) / G as f64];
                if inside(target, p) && !above.iter().any(|s| inside(s, p)) {
                    if p[0] < cx {
                        left += cell;
                    } else {
                        right += cell;
                    }
                }
            }
        }
        (left, right)
    }

    /// Hair a recipe makes about as deep as the head (`d` 1.1) shows beside
    /// the head at the tilted rest pose about as much as it does from the
    /// front, at every avatar size; the same hair as a thin plate (no `d`)
    /// is mostly swallowed by the head's side.
    #[test]
    fn deep_hair_shows_at_rest() {
        let tab = shared_tables();
        for (extent, display) in [(80.47, 52.48), (100.59, 65.6), (120.71, 78.72)] {
            let area = |hair: Option<f64>, rest: bool| {
                let name = rich_body(&bucket_hat3(hair, None)).unwrap();
                let mut mark = Mark::new(&tab, Some(name), 1.0, false, false, 1);
                if !rest {
                    mark.u = [0.0, 0.0, 0.0];
                }
                visible_part(&render_ink(&mark, extent, display, "#1884FC"), "1", "#F6D3B0")
            };
            let (front, thin, deep) = (area(Some(1.1), false), area(None, true), area(Some(1.1), true));
            let sum = |a: (f64, f64)| a.0 + a.1;
            println!(
                "at {display}: hair visible front {:.0}, rest thin {:.0} {thin:.0?}, rest deep {:.0} {deep:.0?}",
                sum(front),
                sum(thin),
                sum(deep)
            );
            assert!(deep.0 > 0.0 && deep.1 > 0.0, "hair on both sides at rest");
            assert!(sum(deep) >= 0.8 * sum(front));
            assert!(sum(deep) >= 1.4 * sum(thin));
        }
    }

    /// Writes the round-6 sheets as SVG into `MARK_DEPTH_DIR` (light
    /// background: the hair is dark).
    #[test]
    fn render_depth_sheets() {
        let Ok(dir) = std::env::var("MARK_DEPTH_DIR") else {
            return;
        };
        let thin = bucket_hat3(None, None);
        let deep = bucket_hat3(Some(1.1), None);
        part_sheets(
            &dir,
            "#D8D8DE",
            &[
                ("blob", "sphere"),
                ("bucket hat (no d)", rich_body(&thin).unwrap()),
                ("bucket hat hair d1.1", rich_body(&deep).unwrap()),
                ("horn hat", rich_body(HORN_HAT).unwrap()),
            ],
        );
    }

    // -- shut eyes and over-layers (round 7) -----------------------------------

    /// A chestnut crop: a skin head with ears, a chestnut crop (d 1.1, z -0.1), the
    /// viewer's right eye shut, a patch on that cheek (an over layer, clear of
    /// the eye), three silver studs on the viewer's left ear.
    const CHESTNUT_CROP: &str = r##"{"o":"M26 56 L20 54 C17 58 18 66 26 68 C30 82 40 90 50 90 C60 90 70 82 74 68 C82 66 83 58 80 54 L74 56 C73 42 62 36 50 36 C38 36 27 42 26 56 Z","c":"#F2C9A0","l":[["M19 59 L21 59 L21 61 L19 61 Z","#C0C7D0"],["M19 62 L21 62 L21 64 L19 64 Z","#C0C7D0"],["M19.5 65 L21.5 65 L21.5 67 L19.5 67 Z","#C0C7D0"],["M58 72 L68 72 L68 78 L58 78 Z","#6B4A3A",1,"over"]],"e":{"c":"#111111","at":[50,62],"gap":18,"s":0.85,"shut":"r"},"p":[{"o":"M24 56 C18 40 32 24 50 24 C68 24 82 40 76 56 C72 46 62 42 50 42 C38 42 28 46 24 56 Z","c":"#7A4E2D","z":-0.1,"d":1.1}]}"##;
    /// A striped eye: the crop's head, the viewer's right eye shut, a thin stripe
    /// across it (an over layer) that crosses the eye without covering it.
    const STRIPED_EYE: &str = r##"{"o":"M26 56 L20 54 C17 58 18 66 26 68 C30 82 40 90 50 90 C60 90 70 82 74 68 C82 66 83 58 80 54 L74 56 C73 42 62 36 50 36 C38 36 27 42 26 56 Z","c":"#F2C9A0","l":[["M50 60.5 L70 60.5 L70 63 L50 63 Z","#6B4A3A",1,"over"]],"e":{"c":"#111111","at":[50,62],"gap":18,"s":0.85,"shut":"r"}}"##;
    /// A teal fringe: a skin head, teal hair (d 1.1, z -0.1), a teal fringe over
    /// the viewer's right eye (over), a straight brow above the left eye and
    /// a small beard.
    const TEAL_FRINGE: &str = r##"{"o":"M25 57 L19 55 C16 59 17 67 25 69 C29 82 39 91 50 91 C61 91 71 82 75 69 C83 67 84 59 81 55 L75 57 C74 43 63 35 50 35 C37 35 26 43 25 57 Z","c":"#F2C9A0","l":[["M34 52 L46 50 L46 52.6 L34 54.6 Z","#1F7F7A"],["M46 84 L54 84 L52 89 L48 89 Z","#1F7F7A"],["M46 34 C60 34 74 42 77 54 C78 64 74 74 66 78 C62 72 58 70 54 68 C52 60 50 50 46 34 Z","#2FA7A0",1,"over"]],"e":{"c":"#111111","at":[50,62],"gap":18,"s":0.85},"p":[{"o":"M22 58 C18 42 32 28 50 28 C68 28 82 42 78 58 C74 46 62 41 50 41 C38 41 26 46 22 58 Z","c":"#2FA7A0","z":-0.1,"d":1.1}]}"##;

    /// The two eyes' loops of a rich tree, viewer's left first.
    fn eye_loops(tree: &MarkTree) -> Vec<Vec<V2>> {
        let mut ps = Vec::new();
        all_paths(tree, &mut ps);
        let mut eyes: Vec<Vec<V2>> = ps
            .iter()
            .filter(|p| p.attr("data-eye").is_some())
            .flat_map(|p| loops(p.attr("d").unwrap_or("")))
            .collect();
        eyes.sort_by(|a, b| centroid(a)[0].total_cmp(&centroid(b)[0]));
        eyes
    }

    fn height(l: &[V2]) -> f64 {
        let (lo, hi) = l.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p[1]), hi.max(p[1])));
        hi - lo
    }

    /// The shut eye is closed in every frame (rest, gaze sweep, every
    /// state clip, pokes); the open eye still opens and blinks.
    #[test]
    fn shut_eye_stays_shut() {
        shut_eye_holds(CHESTNUT_CROP);
    }

    /// An over layer across a shut eye, not covering it: the eye is still
    /// drawn, still shut, and the layer goes over it.
    #[test]
    fn shut_eye_stays_shut_under_an_over_layer() {
        shut_eye_holds(STRIPED_EYE);
        let name = rich_body(STRIPED_EYE).unwrap();
        let tab = shared_tables();
        let tree = render_ink(&Mark::new(&tab, Some(name), 1.0, false, false, 1), 120.71, 78.72, "#1884FC");
        let mut ps = Vec::new();
        all_paths(&tree, &mut ps);
        let eye = ps.iter().rposition(|p| p.attr("data-eye").is_some()).expect("eyes");
        let stripe = ps.iter().position(|p| p.attr("fill") == Some("#6B4A3A")).expect("the stripe");
        assert!(stripe > eye, "the stripe draws over the eye");
    }

    fn shut_eye_holds(recipe: &str) {
        let tab = shared_tables();
        let name = rich_body(recipe).unwrap();
        let open = eye_loops(&render_ink(&Mark::new(&tab, Some(name), 1.0, false, false, 1), 120.71, 78.72, "#1884FC"));
        assert_eq!(open.len(), 2);
        let (left_open, right_shut) = (height(&open[0]), height(&open[1]));
        println!("rest: open eye {left_open:.1}, shut eye {right_shut:.1}");
        assert!(right_shut < 0.3 * left_open);
        let rest_area = signed_area(&open[0]).abs();
        let mut frames = 0;
        let mut worst = 0.0f64;
        for (label, tree, _, _) in rich_run(name, 120.71, 78.72) {
            let eyes = eye_loops(&tree);
            if eyes.len() == 2 {
                frames += 1;
                // the viewer's right eye is the closed lid in every frame,
                // whatever the expression widens the eyes to: a fraction of
                // the open eye, or no bigger than it while it blinks too
                let (open, shut) = (signed_area(&eyes[0]).abs(), signed_area(&eyes[1]).abs());
                let ratio = shut / open;
                if open > 0.5 * rest_area {
                    worst = worst.max(ratio);
                    assert!(ratio < 0.4, "{label}: shut eye {shut:.0} vs open {open:.0}");
                } else {
                    assert!(shut <= open * 1.25 + 1.0, "{label}: shut eye {shut:.0} vs blinking {open:.0}");
                }
            }
        }
        println!("shut eye at most {worst:.2} of the open eye's area over {frames} frames");
        assert!(frames > 1000);
        let blink = blink_mark(name);
        let eyes = eye_loops(&render_ink(&blink, 120.71, 78.72, "#1884FC"));
        let open_now = eyes.iter().map(|e| height(e)).fold(0.0, f64::max);
        println!("blink: open eye {open_now:.1}");
        assert!(open_now < 0.4 * left_open);
    }

    /// An over-layer draws above the eyes, and an eye under it stays
    /// covered through the gaze sweep, blinks and every state clip: an eye
    /// the layer covers in the design is not drawn at all.
    #[test]
    fn over_layer_hides_the_eye() {
        let name = rich_body(TEAL_FRINGE).unwrap();
        let mut covered = 0;
        for (label, tree, _, morph) in rich_run(name, 120.71, 78.72) {
            let mut ps = Vec::new();
            all_paths(&tree, &mut ps);
            let Some(at) = ps.iter().position(|p| p.attr("data-layer") == Some("2")) else {
                continue;
            };
            let last_eye = ps.iter().rposition(|p| p.attr("data-eye").is_some());
            assert!(last_eye.is_none_or(|e| e < at), "{label}: the fringe draws under the eyes");
            let eyes = eye_loops(&tree);
            if morph {
                continue;
            }
            // the viewer's right eye sits under the fringe: never drawn
            assert!(eyes.len() <= 1, "{label}: {} eyes", eyes.len());
            if label.starts_with("rest") {
                assert_eq!(eyes.len(), 1, "{label}: the open eye");
            }
            covered += 1;
        }
        println!("eye covered in {covered} frames");
        assert!(covered > 1000);
    }

    #[test]
    fn shut_and_over_parse() {
        let head = r##""o":"M24 64 C24 46 36 38 50 38 C64 38 76 46 76 64 C76 80 64 92 50 92 C36 92 24 80 24 64 Z","c":"#141414""##;
        assert!(rich_body(&format!(r##"{{{head},"e":{{"c":"#FFFFFF","shut":"x"}}}}"##)).is_err());
        assert!(rich_body(&format!(r##"{{{head},"e":"#FFFFFF","l":[["M30 50 L60 50 L60 60 Z","#FFFFFF",1,"under"]]}}"##)).is_err());
        let a = rich(rich_body(&format!(r##"{{{head},"e":"#FFFFFF","l":[["M30 50 L60 50 L60 60 Z","#FFFFFF","over"]]}}"##)).unwrap()).unwrap();
        let b = rich(rich_body(&format!(r##"{{{head},"e":"#FFFFFF","l":[{{"path":"M30 50 L60 50 L60 60 Z","fill":"#FFFFFF","over":true}}]}}"##)).unwrap()).unwrap();
        assert!(a.decals[0].over && b.decals[0].over);
        assert_eq!(a.decals[0].loops, b.decals[0].loops);
        let c = rich(rich_body(&format!(r##"{{{head},"e":{{"c":"#FFFFFF","shut":"l"}}}}"##)).unwrap()).unwrap();
        assert_eq!(c.shut, Some(0));
    }

    /// Writes the round-7 sheets as SVG into `MARK_SHUT_DIR`, plus a blink.
    #[test]
    fn render_shut_sheets() {
        let Ok(dir) = std::env::var("MARK_SHUT_DIR") else {
            return;
        };
        part_sheets(
            &dir,
            "#D8D8DE",
            &[("blob", "sphere"), ("chestnut crop", rich_body(CHESTNUT_CROP).unwrap()), ("teal fringe", rich_body(TEAL_FRINGE).unwrap())],
        );
        let mut svg = String::from("<svg xmlns='http://www.w3.org/2000/svg' width='330' height='170'><rect width='100%' height='100%' fill='#D8D8DE'/>");
        for (c, recipe) in [CHESTNUT_CROP, TEAL_FRINGE].iter().enumerate() {
            let mark = blink_mark(rich_body(recipe).unwrap());
            let mut t = render_ink(&mark, 144.0, 96.0, "#1884FC");
            t.attrs.retain(|(k, _)| *k != "id");
            let mut x = String::new();
            xml(&t, &mut x);
            svg += &format!("<g transform='translate({} 10)'>{}</g>", 10.0 + c as f64 * 160.0, x.replace("m-", &format!("b{c}-")));
        }
        std::fs::write(format!("{dir}/blink.svg"), svg + "</svg>").unwrap();
    }

}
