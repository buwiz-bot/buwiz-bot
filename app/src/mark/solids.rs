//! Solid accessories: real 3D geometry attached to a rich body's head.
//!
//! A rich body's parts (`custom::multi_part`) are outlines facing the
//! camera, puffed into pillows, so a hat brim turns edge-on into a leaf and
//! hair behind the head vanishes side-on. A solid is defined in head space
//! instead, and turns with the head:
//!
//! - a **lathe**: a profile revolved around an axis (a brim with its crown,
//!   a cap, a band, a jewelled crown, an ear);
//! - a **shell**: a surface offset from the head, covering it down to a
//!   hairline with a scalloped edge, with rounded lobes around it for big
//!   hair;
//! - a **tube**: a tapering tube swept along a path, with branches (antlers,
//!   plumes, spikes).
//!
//! All of them become one [`Scene`] of faces in body units, built once when
//! the recipe registers. Drawing it is a painter's pass per pose:
//!
//! 1. A face that looks away is dropped. Every surface is closed (a hat's
//!    profile runs back under it to the axis), so one always stands in front.
//!    So is a face a lathe's own widest ring hides (see [`Ring`]).
//! 2. Every face is near or far: in front of the plane through the head's
//!    centre that faces the viewer, or behind it. Far faces draw before the
//!    head and near ones after it, so one brim passes in front of the head
//!    on one side and behind it on the other. The head is about convex and
//!    no face sits inside it (a face wholly inside the head or another
//!    solid is dropped at build time, and one that enters the head is cut
//!    back to its surface), so this plane decides right.
//! 3. Within a half, faces draw in order of distance from the head's
//!    centre: outward in the near half, inward in the far half. Two points
//!    on one line of sight and one side of the plane differ only in depth,
//!    so the one farther from the centre is the nearer (or, behind, the
//!    farther). The order is the same at every pose, so it's worked out
//!    once and nothing flickers as the head turns. Faces in one step of
//!    distance can only meet near a seam, so they draw as one group: each
//!    colour of a step is one path, and a hat costs a few dozen paths
//!    instead of a few hundred faces.
//!
//! A face takes one of three flat tones from its normal against a fixed
//! light, the toon version of shading. Marks (a hat band, a cross, jewels)
//! are drawn in a surface's own chart, `u` degrees around it and `v` percent
//! along it. Each is cut to the grid at build time, so its pieces ride the
//! faces, turn away with them and take their tone.

use serde_json::Value;

use super::bodies::{self, Rot};
use super::{V2, V3};

const TAU: f64 = std::f64::consts::TAU;
const PI: f64 = std::f64::consts::PI;

/// The most solids a recipe takes.
pub(crate) const MAX_SOLIDS: usize = 10;
/// The most points in a lathe's profile.
const MAX_PROFILE: usize = 40;
/// The most points in a tube's path or a branch's.
const MAX_PATH: usize = 12;
/// The most branches on a tube.
const MAX_BRANCHES: usize = 6;
/// The most marks on a solid.
const MAX_MARKS: usize = 8;
/// The longest mark path.
const MAX_MARK_PATH: usize = 4000;
/// A mark's chart, `u` degrees round and `v` percent along, with the margin
/// the layers' box has.
pub(super) const CHART_U: (f64, f64) = (-180.0, 180.0);
pub(super) const CHART_V: (f64, f64) = (-5.0, 105.0);
/// How finely a mark's path is sampled, in chart units. Its straight edges
/// are cut at every cell anyway, so this only has to follow its curves.
pub(super) const MARK_STEP: f64 = 2.0;
/// The most sampled points in a mark and in all of a solid's. A short path
/// can retrace a long edge hundreds of times, and every point is clipped
/// against every face, so a path's length in characters is no limit.
const MAX_MARK_POINTS: usize = 1000;
const MAX_SOLID_MARK_POINTS: usize = 2000;
/// The most control points in a hairline.
const MAX_EDGE: usize = 24;
/// The most scallops around a hairline.
const MAX_SCALLOPS: usize = 18;
/// The most lobes on a shell.
const MAX_LOBES: usize = 32;
/// The most faces a recipe's solids may cost (see [`Spec::cost`]): what keeps
/// a bad recipe from stalling the renderer.
const MAX_FACES: usize = 8000;
/// Head-space coordinates stay inside this range (the 100×100 box and a
/// wide margin).
const BOX_RANGE: (f64, f64) = (-60.0, 160.0);
/// The widest radius or offset, in box units.
const MAX_RADIUS: f64 = 60.0;

/// The step of distance from the head's centre (body units) that draws as
/// one group.
const STEP: f64 = 0.035;
/// How finely the solids are cut.
#[derive(Clone, Copy)]
struct Detail {
    /// Columns around a lathe (fewer for a small one).
    lathe_cols: usize,
    /// The longest step along a profile (box units): a longer one is split,
    /// so that no face spans more distance from the head's centre than the
    /// order can stand.
    lathe_row: f64,
    /// Rows from the crown of a shell to its hairline.
    shell_rows: usize,
    /// Columns around a shell without scallops, and at most with them.
    shell_cols: usize,
    shell_max_cols: usize,
    /// Columns per scallop, at least.
    scallop_cols: usize,
    /// A lobe's rows and columns.
    lobe_rows: usize,
    lobe_cols: usize,
    /// A tube's sides, and its rings per span of the path.
    tube_sides: usize,
    tube_steps: usize,
}

const FINE: Detail = Detail {
    lathe_cols: 32,
    lathe_row: 5.0,
    shell_rows: 6,
    shell_cols: 40,
    shell_max_cols: 54,
    scallop_cols: 4,
    lobe_rows: 7,
    lobe_cols: 12,
    tube_sides: 8,
    tube_steps: 3,
};
/// For a small avatar: a sidebar's has about 30pt to show it in.
const COARSE: Detail = Detail {
    lathe_cols: 24,
    lathe_row: 7.0,
    shell_rows: 5,
    shell_cols: 28,
    shell_max_cols: 36,
    scallop_cols: 2,
    lobe_rows: 5,
    lobe_cols: 8,
    tube_sides: 6,
    tube_steps: 2,
};
/// Below this size (px of the mark's sprite) a mark draws [`COARSE`].
const SMALL: f64 = 48.0;

/// Halvings to find where an edge meets the head: past a thousandth of a
/// pixel at any size an avatar draws.
const CUT_STEPS: usize = 20;

/// The direction the light comes from, in view space (x right, y up, z
/// toward the viewer): high on the viewer's left.
const LIGHT: V3 = [-0.42, 0.72, 0.55];
/// Faces at least this lit take the lit tone, and at least [`MID`] the mid.
const LIT: f64 = 0.38;
const MID: f64 = -0.12;
/// Faces turned less than this (the cosine) from their neighbour shade as
/// one smooth surface; past it the edge is a crease.
const CREASE: f64 = 0.6;

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn lerp3(a: V3, b: V3, t: f64) -> V3 {
    add(a, scale(sub(b, a), t))
}
fn length(a: V3) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: V3) -> V3 {
    let l = length(a);
    if l > 1e-12 {
        scale(a, 1.0 / l)
    } else {
        [0.0, 0.0, 0.0]
    }
}

// ---------------------------------------------------------------------------
// the head's surface
// ---------------------------------------------------------------------------

/// The head's loft as a surface around its centre: what a shell wraps, and
/// what hides a face inside it.
pub(super) struct Head {
    center: V3,
    depth: f64,
    /// The outline's radius from the centre, by angle (even steps round).
    polar: Vec<f64>,
    /// The loft's rings by even steps of their angle: each ring's height
    /// over its scale (rising without end), and its scale.
    rings: Vec<(f64, f64)>,
}

/// The loft's ring exponent (`bodies::ss`): a ring at angle `a` is the
/// outline scaled by `cos(a)^0.72`.
const LOFT_POWER: f64 = 0.72;

impl Head {
    /// The surface of the loft of `points` (`Jt` points) to `±depth`.
    pub(super) fn new(points: &[V3], depth: f64) -> Head {
        let (mut x0, mut x1, mut y0, mut y1) =
            (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
        for p in points {
            x0 = x0.min(p[0]);
            x1 = x1.max(p[0]);
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1]);
        }
        let c = [(x0 + x1) / 2.0, (y0 + y1) / 2.0];
        const N: usize = 360;
        let n = points.len();
        let polar = (0..N)
            .map(|i| {
                let t = TAU * i as f64 / N as f64;
                let (ux, uy) = (t.cos(), t.sin());
                let mut best = 0.0f64;
                for k in 0..n {
                    let (a, b) = (points[k], points[(k + 1) % n]);
                    let (ax, ay) = (a[0] - c[0], a[1] - c[1]);
                    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                    let den = ux * dy - uy * dx;
                    if den.abs() < 1e-12 {
                        continue;
                    }
                    let s = (ax * dy - ay * dx) / den;
                    let along = (ax * uy - ay * ux) / den;
                    if s > best && (0.0..=1.0).contains(&along) {
                        best = s;
                    }
                }
                best
            })
            .collect();
        const RINGS: usize = 256;
        let rings = (0..RINGS)
            .map(|i| {
                let a = PI / 2.0 * i as f64 / RINGS as f64;
                let f = a.cos().powf(LOFT_POWER);
                (a.sin() / f, f)
            })
            .collect();
        Head {
            center: [c[0], c[1], 0.0],
            depth,
            polar,
            rings,
        }
    }

    fn rho(&self, t: f64) -> f64 {
        let n = self.polar.len();
        let f = t.rem_euclid(TAU) / TAU * n as f64;
        let i = f.floor() as usize % n;
        let k = f - f.floor();
        self.polar[i] * (1.0 - k) + self.polar[(i + 1) % n] * k
    }

    /// The distance from the centre to the surface along unit `d`.
    fn radius(&self, d: V3) -> f64 {
        let b = d[0].hypot(d[1]);
        if b < 1e-9 {
            return self.depth;
        }
        let rho = self.rho(d[1].atan2(d[0]));
        // the ring this direction meets: height over scale is the slope
        // of the direction, in the loft's own proportions
        let slope = d[2].abs() / b * rho / self.depth;
        // past the last ring: on the head's axis, where the surface is its depth
        if self.rings.last().is_some_and(|r| slope >= r.0) {
            return self.depth;
        }
        let i = self.rings.partition_point(|r| r.0 < slope).min(self.rings.len() - 1);
        let (lo, hi) = (self.rings[i.saturating_sub(1)], self.rings[i]);
        let t = if hi.0 > lo.0 { ((slope - lo.0) / (hi.0 - lo.0)).clamp(0.0, 1.0) } else { 0.0 };
        let f = lo.1 + (hi.1 - lo.1) * t;
        (f * rho).hypot(f * slope * self.depth)
    }

    fn contains(&self, p: V3) -> bool {
        let v = sub(p, self.center);
        let l = length(v);
        l < 1e-9 || l < self.radius(scale(v, 1.0 / l)) - 1e-3
    }

    /// The direction `pol` degrees from the top, `az` degrees round from
    /// the front toward the viewer's right.
    fn direction(az: f64, pol: f64) -> V3 {
        let (a, p) = (az.to_radians(), pol.to_radians());
        [p.sin() * a.sin(), p.cos(), p.sin() * a.cos()]
    }
}

// ---------------------------------------------------------------------------
// the recipe
// ---------------------------------------------------------------------------

/// A mark: closed loops in a surface's chart, `[u, v]`.
struct Mark {
    loops: Vec<Vec<V2>>,
    /// Each loop's box, `[u0, v0, u1, v1]`.
    bounds: Vec<[f64; 4]>,
    fill: String,
}

/// A tube's point: x, y, z in the box and its radius there.
type TubePoint = [f64; 4];

enum Kind {
    Lathe {
        at: V3,
        tilt: V3,
        profile: Vec<V2>,
        oval: f64,
        /// How many points what's above height 0 rises to, and how far down
        /// the valleys between them cut (0..1): a crown's points.
        teeth: Option<(usize, f64)>,
    },
    Shell {
        offset: f64,
        /// The hairline, `[azimuth, degrees from the top]`, azimuth rising.
        edge: Vec<V2>,
        /// How many scallops, how deep (degrees), and whether pointed.
        scallop: Option<(usize, f64, bool)>,
        /// How many lobes, their radius, and how far off the shell they sit.
        lobes: Option<(usize, f64, f64)>,
    },
    Tube {
        path: Vec<TubePoint>,
        branches: Vec<Vec<TubePoint>>,
    },
}

/// Reads a `#rrggbb`, naming what it's for in the error.
type Colour<'a> = &'a dyn Fn(Option<&Value>, &str) -> Result<String, String>;
/// Reads a mark's path as closed loops.
type Chart<'a> = &'a dyn Fn(&str) -> Result<Vec<Vec<V2>>, String>;

/// One solid of a recipe.
pub(super) struct Spec {
    kind: Kind,
    color: String,
    marks: Vec<Mark>,
}

fn number(o: &Value, key: &str, what: &str) -> Result<Option<f64>, String> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|v| v.is_finite())
            .map(Some)
            .ok_or_else(|| format!("{what} \"{key}\" must be a number")),
    }
}

fn numbers(v: &Value, n: usize, what: &str) -> Result<Vec<f64>, String> {
    let a = v.as_array().ok_or_else(|| format!("{what} must be a list of {n} numbers"))?;
    if a.len() != n {
        return Err(format!("{what} must be a list of {n} numbers"));
    }
    a.iter()
        .map(|x| {
            x.as_f64()
                .filter(|x| x.is_finite())
                .ok_or_else(|| format!("{what} must be a list of {n} numbers"))
        })
        .collect()
}

fn in_box(v: f64) -> bool {
    (BOX_RANGE.0..=BOX_RANGE.1).contains(&v)
}

fn tube_path(v: &Value, what: &str) -> Result<Vec<TubePoint>, String> {
    let a = v.as_array().ok_or_else(|| format!("{what} must be a list of [x, y, z, radius]"))?;
    if !(2..=MAX_PATH).contains(&a.len()) {
        return Err(format!("{what} needs 2 to {MAX_PATH} points"));
    }
    a.iter()
        .map(|p| {
            let n = numbers(p, 4, &format!("{what}'s point"))?;
            if !n[..3].iter().all(|v| in_box(*v)) || !(0.0..=MAX_RADIUS).contains(&n[3]) {
                return Err(format!("{what}'s point is out of range"));
            }
            Ok([n[0], n[1], n[2], n[3]])
        })
        .collect()
}

impl Spec {
    /// One solid of the recipe's `"s"`, with `colour` reading a `#rrggbb`
    /// and `chart` a mark's path as closed loops.
    fn parse(
        v: &Value,
        n: usize,
        colour: Colour,
        chart: Chart,
    ) -> Result<Spec, String> {
        let what = format!("solid {n}");
        if !v.is_object() {
            return Err(format!("{what} must be an object"));
        }
        let color = colour(v.get("color"), &what)?;
        let kind = match v.get("kind").and_then(|k| k.as_str()) {
            Some("lathe") => {
                let at = match v.get("at") {
                    None | Some(Value::Null) => return Err(format!("{what} needs \"at\"")),
                    Some(a) => numbers(a, 3, &format!("{what} \"at\""))?,
                };
                if !at.iter().all(|v| in_box(*v)) {
                    return Err(format!("{what} \"at\" is out of range"));
                }
                let tilt = match v.get("tilt") {
                    None | Some(Value::Null) => vec![0.0; 3],
                    Some(t) => numbers(t, 3, &format!("{what} \"tilt\""))?,
                };
                if !tilt.iter().all(|t| t.abs() <= 180.0) {
                    return Err(format!("{what} \"tilt\" must be -180..180"));
                }
                let list = v
                    .get("profile")
                    .and_then(|p| p.as_array())
                    .ok_or_else(|| format!("{what} needs a \"profile\""))?;
                if !(2..=MAX_PROFILE).contains(&list.len()) {
                    return Err(format!("{what}'s profile needs 2 to {MAX_PROFILE} points"));
                }
                let mut profile = Vec::with_capacity(list.len());
                for p in list {
                    let q = numbers(p, 2, &format!("{what}'s profile point"))?;
                    if !(0.0..=MAX_RADIUS * 2.0).contains(&q[0]) || q[1].abs() > MAX_RADIUS * 2.0 {
                        return Err(format!("{what}'s profile point is out of range"));
                    }
                    profile.push([q[0], q[1]]);
                }
                // closed: axis to axis, or back to its start
                let (first, last) = (profile[0], profile[profile.len() - 1]);
                if !(first == last || (first[0] == 0.0 && last[0] == 0.0)) {
                    return Err(format!("{what}'s profile must run axis to axis, or back to its start"));
                }
                let oval = number(v, "oval", &what)?.unwrap_or(1.0);
                if !(0.3..=3.0).contains(&oval) {
                    return Err(format!("{what} \"oval\" must be 0.3..3"));
                }
                let teeth = match v.get("teeth") {
                    None | Some(Value::Null) => None,
                    Some(s) => {
                        let q = numbers(s, 2, &format!("{what} \"teeth\""))?;
                        if q[0].fract() != 0.0 || !(2.0..=MAX_SCALLOPS as f64).contains(&q[0]) || !(0.0..=1.0).contains(&q[1]) {
                            return Err(format!("{what} \"teeth\" must be [2..{MAX_SCALLOPS}, 0..1]"));
                        }
                        Some((q[0] as usize, q[1]))
                    }
                };
                Kind::Lathe {
                    at: [at[0], at[1], at[2]],
                    tilt: [tilt[0], tilt[1], tilt[2]],
                    profile,
                    oval,
                    teeth,
                }
            }
            Some("shell") => {
                let offset = number(v, "offset", &what)?.unwrap_or(2.0);
                if !(0.0..=MAX_RADIUS).contains(&offset) {
                    return Err(format!("{what} \"offset\" must be 0..{MAX_RADIUS}"));
                }
                let list = v
                    .get("edge")
                    .and_then(|e| e.as_array())
                    .ok_or_else(|| format!("{what} needs an \"edge\""))?;
                if !(1..=MAX_EDGE).contains(&list.len()) {
                    return Err(format!("{what}'s edge needs 1 to {MAX_EDGE} points"));
                }
                let mut edge: Vec<V2> = Vec::with_capacity(list.len());
                for p in list {
                    let q = numbers(p, 2, &format!("{what}'s edge point"))?;
                    if q[0].abs() > 180.0 || !(5.0..=175.0).contains(&q[1]) {
                        return Err(format!("{what}'s edge point must be [-180..180, 5..175]"));
                    }
                    if edge.last().is_some_and(|l| l[0] >= q[0]) {
                        return Err(format!("{what}'s edge azimuths must rise"));
                    }
                    edge.push([q[0], q[1]]);
                }
                let scallop = match v.get("scallop") {
                    None | Some(Value::Null) => None,
                    Some(s) => {
                        let q = numbers(s, 3, &format!("{what} \"scallop\""))?;
                        if q[0].fract() != 0.0
                            || !(2.0..=MAX_SCALLOPS as f64).contains(&q[0])
                            || !(0.0..=60.0).contains(&q[1])
                            || (q[2] != 0.0 && q[2] != 1.0)
                        {
                            return Err(format!(
                                "{what} \"scallop\" must be [2..{MAX_SCALLOPS}, 0..60, 0 or 1]"
                            ));
                        }
                        Some((q[0] as usize, q[1], q[2] == 1.0))
                    }
                };
                let lobes = match v.get("lobes") {
                    None | Some(Value::Null) => None,
                    Some(s) => {
                        let q = numbers(s, 3, &format!("{what} \"lobes\""))?;
                        if q[0].fract() != 0.0
                            || !(1.0..=MAX_LOBES as f64).contains(&q[0])
                            || !(1.0..=MAX_RADIUS).contains(&q[1])
                            || q[2].abs() > MAX_RADIUS
                        {
                            return Err(format!(
                                "{what} \"lobes\" must be [1..{MAX_LOBES}, 1..{MAX_RADIUS}, -{MAX_RADIUS}..{MAX_RADIUS}]"
                            ));
                        }
                        Some((q[0] as usize, q[1], q[2]))
                    }
                };
                Kind::Shell {
                    offset,
                    edge,
                    scallop,
                    lobes,
                }
            }
            Some("tube") => {
                let path = tube_path(
                    v.get("path").ok_or_else(|| format!("{what} needs a \"path\""))?,
                    &format!("{what}'s path"),
                )?;
                let branches = match v.get("branches") {
                    None | Some(Value::Null) => vec![],
                    Some(b) => {
                        let a = b.as_array().ok_or_else(|| format!("{what} \"branches\" must be a list"))?;
                        if a.len() > MAX_BRANCHES {
                            return Err(format!("{what} has more than {MAX_BRANCHES} branches"));
                        }
                        a.iter()
                            .map(|p| tube_path(p, &format!("{what}'s branch")))
                            .collect::<Result<Vec<_>, _>>()?
                    }
                };
                Kind::Tube { path, branches }
            }
            _ => return Err(format!("{what} \"kind\" must be lathe, shell or tube")),
        };
        let marks = match v.get("marks") {
            None | Some(Value::Null) => vec![],
            Some(m) => {
                let a = m.as_array().ok_or_else(|| format!("{what} \"marks\" must be a list"))?;
                if a.len() > MAX_MARKS {
                    return Err(format!("{what} has more than {MAX_MARKS} marks"));
                }
                let mut marks = Vec::with_capacity(a.len());
                let mut total = 0;
                for (i, m) in a.iter().enumerate() {
                    let name = format!("{what} mark {}", i + 1);
                    let path = m
                        .get("path")
                        .and_then(|p| p.as_str())
                        .filter(|p| p.len() <= MAX_MARK_PATH)
                        .ok_or_else(|| format!("{name} needs a path of at most {MAX_MARK_PATH} characters"))?;
                    let loops = chart(path).map_err(|e| format!("{name}: {e}"))?;
                    if loops.is_empty() {
                        return Err(format!("{name} encloses no area"));
                    }
                    let points = loops.iter().map(Vec::len).sum::<usize>();
                    if points > MAX_MARK_POINTS {
                        return Err(format!("{name} samples to {points} points, at most {MAX_MARK_POINTS}"));
                    }
                    total += points;
                    if total > MAX_SOLID_MARK_POINTS {
                        return Err(format!("{what}'s marks sample to {total} points, at most {MAX_SOLID_MARK_POINTS}"));
                    }
                    let bounds = loops
                        .iter()
                        .map(|l| {
                            l.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| {
                                [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
                            })
                        })
                        .collect();
                    marks.push(Mark {
                        loops,
                        bounds,
                        fill: colour(m.get("fill"), &name)?,
                    });
                }
                marks
            }
        };
        Ok(Spec { kind, color, marks })
    }

    /// What the solid costs, in faces at most and its marks' points: the
    /// number [`MAX_FACES`] caps, the same sum `avatar-shape` makes.
    fn cost(&self) -> usize {
        let tube = |p: &Vec<TubePoint>| ((p.len() - 1) * FINE.tube_steps + 3) * FINE.tube_sides;
        let marks: usize = self.marks.iter().flat_map(|m| &m.loops).map(Vec::len).sum();
        marks + match &self.kind {
            Kind::Lathe { profile, .. } => (cut_profile(profile, FINE.lathe_row).len() - 1) * (FINE.lathe_cols + 2 * MAX_SCALLOPS),
            Kind::Shell { lobes, .. } => {
                // the most scallops, at their least columns each, is the
                // widest a shell gets
                FINE.shell_rows * MAX_SCALLOPS * FINE.scallop_cols + lobes.map_or(0, |l| l.0 * FINE.lobe_rows * FINE.lobe_cols)
            }
            Kind::Tube { path, branches } => tube(path) + branches.iter().map(tube).sum::<usize>(),
        }
    }

    /// Points that bound the solid in the box (x, y, z): the frame is
    /// fitted to them. `head` is the head outline's bounding box.
    fn reach(&self, head: [f64; 4]) -> Vec<V3> {
        match &self.kind {
            Kind::Lathe { at, tilt, profile, oval, .. } => {
                let m = rotation(*tilt);
                let mut out = Vec::new();
                for p in profile {
                    for i in 0..16 {
                        let a = TAU * i as f64 / 16.0;
                        let q = apply(&m, [p[0] * a.sin(), p[1], p[0] * a.cos() * oval]);
                        out.push([at[0] + q[0], at[1] - q[1], at[2] + q[2]]);
                    }
                }
                out
            }
            Kind::Shell { offset, lobes, .. } => {
                let grow = offset + lobes.map_or(0.0, |l| (l.2 + l.1 * 1.15).max(0.0));
                let [x0, y0, x1, y1] = head;
                let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
                let (rx, ry) = ((x1 - x0) / 2.0 + grow, (y1 - y0) / 2.0 + grow);
                (0..16)
                    .map(|i| {
                        let a = TAU * i as f64 / 16.0;
                        [cx + rx * a.cos(), cy + ry * a.sin(), 0.0]
                    })
                    .collect()
            }
            Kind::Tube { path, branches } => std::iter::once(path)
                .chain(branches)
                .flatten()
                .flat_map(|p| {
                    [
                        [p[0] - p[3], p[1], p[2]],
                        [p[0] + p[3], p[1], p[2]],
                        [p[0], p[1] - p[3], p[2]],
                        [p[0], p[1] + p[3], p[2]],
                    ]
                })
                .collect(),
        }
    }
}

/// `profile` with its steps longer than `row` split.
fn cut_profile(profile: &[V2], row: f64) -> Vec<V2> {
    let mut out = vec![profile[0]];
    for w in profile.windows(2) {
        let n = ((w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]) / row).ceil().max(1.0) as usize;
        for i in 1..=n {
            let t = i as f64 / n as f64;
            out.push([w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t]);
        }
    }
    out
}

/// The recipe's solids.
pub(super) fn parse(
    list: &Value,
    colour: Colour,
    chart: Chart,
) -> Result<Vec<Spec>, String> {
    let a = list.as_array().ok_or("the solids (\"s\") must be a list")?;
    if a.len() > MAX_SOLIDS {
        return Err(format!("at most {MAX_SOLIDS} solids, got {}", a.len()));
    }
    let specs: Vec<Spec> = a
        .iter()
        .enumerate()
        .map(|(i, v)| Spec::parse(v, i + 1, colour, chart))
        .collect::<Result<_, _>>()?;
    let cost: usize = specs.iter().map(Spec::cost).sum();
    if cost > MAX_FACES {
        return Err(format!("the solids cost {cost} faces, at most {MAX_FACES}"));
    }
    Ok(specs)
}

/// Every point bounding `specs` in the box.
pub(super) fn reach(specs: &[Spec], head: [f64; 4]) -> Vec<V3> {
    specs.iter().flat_map(|s| s.reach(head)).collect()
}

/// `tilt` (degrees about x, y, z, in that order) as a rotation matrix.
fn rotation(tilt: V3) -> [V3; 3] {
    let (a, b, c) = (tilt[0].to_radians(), tilt[1].to_radians(), tilt[2].to_radians());
    let (ca, sa, cb, sb, cc, sc) = (a.cos(), a.sin(), b.cos(), b.sin(), c.cos(), c.sin());
    // Rz · Ry · Rx
    [
        [cc * cb, cc * sb * sa - sc * ca, cc * sb * ca + sc * sa],
        [sc * cb, sc * sb * sa + cc * ca, sc * sb * ca - cc * sa],
        [-sb, cb * sa, cb * ca],
    ]
}

fn apply(m: &[V3; 3], p: V3) -> V3 {
    [dot(m[0], p), dot(m[1], p), dot(m[2], p)]
}

fn transpose(m: &[V3; 3]) -> [V3; 3] {
    [
        [m[0][0], m[1][0], m[2][0]],
        [m[0][1], m[1][1], m[2][1]],
        [m[0][2], m[1][2], m[2][2]],
    ]
}

// ---------------------------------------------------------------------------
// the mesh
// ---------------------------------------------------------------------------

/// A surface as a grid of points, rows along it, columns around it, with
/// each row and column's place in the chart.
struct Grid {
    rows: Vec<Vec<V3>>,
    u: Vec<f64>,
    v: Vec<f64>,
}

/// A piece of a mark on one face, in body units.
struct Fragment {
    points: Vec<V3>,
    /// The surface's normal at each point.
    normals: Vec<V3>,
    paint: usize,
}

struct Face {
    /// Corners, indices into [`Scene::points`].
    corners: [u32; 4],
    normal: V3,
    /// The smooth surface's normal at each corner: tones change along a
    /// curve across the faces instead of stepping from face to face.
    shading: [V3; 4],
    centre: V3,
    paint: usize,
    /// The ring of its own lathe it sits above (an index into
    /// [`Scene::rings`]).
    above: Option<usize>,
    fragments: Vec<Fragment>,
}

/// Where the edge from `inside` the head to `outside` it meets the surface,
/// as a fraction of the way along.
fn crossing(head: &Head, inside: V3, outside: V3) -> f64 {
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..CUT_STEPS {
        let mid = (lo + hi) / 2.0;
        if head.contains(lerp3(inside, outside, mid)) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

/// `face` with what's inside the head cut away, as one face or two. Left
/// whole, a face drawn after the head shows the part that should be inside
/// it, and a solid sinking into the head ends in stairs a face wide.
///
/// A face carrying marks stays whole: they're cut to its shape already.
fn cut_at_head(face: Face, points: &mut Vec<V3>, head: &Head) -> Vec<Face> {
    let corners = face.corners.map(|c| points[c as usize]);
    let buried = corners.map(|p| head.contains(p));
    if !face.fragments.is_empty() || buried.iter().all(|b| *b) || !buried.iter().any(|b| *b) {
        return vec![face];
    }
    let cross = |from: usize, to: usize, points: &mut Vec<V3>| -> (u32, V3) {
        let t = crossing(head, corners[from], corners[to]);
        let at = u32::try_from(points.len()).expect("a scene's points fit u32");
        points.push(lerp3(corners[from], corners[to], t));
        (at, unit(lerp3(face.shading[from], face.shading[to], t)))
    };
    // each run of corners left is its own piece, between its two crossings:
    // buried corners can alternate, and what lies between two runs is inside
    let mut pieces = Vec::with_capacity(2);
    for buried_one in 0..4 {
        let first = (buried_one + 1) % 4;
        if !buried[buried_one] || buried[first] {
            continue;
        }
        let mut run = vec![cross(buried_one, first, points)];
        let mut i = first;
        while !buried[i] {
            run.push((face.corners[i], face.shading[i]));
            let next = (i + 1) % 4;
            if buried[next] {
                run.push(cross(next, i, points));
            }
            i = next;
        }
        fan(&run, &face, points, &mut pieces);
    }
    pieces
}

/// `run`, a convex outline of corners and their normals, as faces like `face`:
/// quads fanned from its first corner. A piece of three repeats its last.
fn fan(run: &[(u32, V3)], face: &Face, points: &[V3], pieces: &mut Vec<Face>) {
    let last = run.len() - 1;
    let mut start = 1;
    while start < last {
        let (b, c, d) = (start, start + 1, (start + 2).min(last));
        let piece = [run[0], run[b], run[c], run[d]];
        // the centre sorts the face: a repeated corner mustn't pull it over
        let distinct = if d == c { 3 } else { 4 };
        let sum = piece[..distinct].iter().fold([0.0; 3], |sum, (i, _)| add(sum, points[*i as usize]));
        pieces.push(Face {
            corners: piece.map(|(i, _)| i),
            normal: face.normal,
            shading: piece.map(|(_, n)| n),
            centre: scale(sum, 1.0 / distinct as f64),
            paint: face.paint,
            above: face.above,
            fragments: Vec::new(),
        });
        start += 2;
    }
}

/// A lathe's widest ring. From below it, the disc it spans hides what's
/// above it: a brim hides its crown's wall. The two meet at a crease, where
/// sorting by distance can't tell them apart, so this is worked out exactly.
struct Ring {
    at: V3,
    to_local: [V3; 3],
    oval: f64,
    height: f64,
    radius: f64,
}

impl Ring {
    /// Whether the disc stands between `p` and a viewer along `view`.
    fn hides(&self, p: V3, view: V3) -> bool {
        let v = apply(&self.to_local, view);
        if v[1] >= -1e-3 {
            return false;
        }
        let q = apply(&self.to_local, sub(p, self.at));
        let t = (q[1] - self.height) / -v[1];
        (q[0] + v[0] * t).hypot((q[2] + v[2] * t) / self.oval) < self.radius * 0.995
    }
}

/// A surface's volume, for dropping the faces inside it.
enum Volume {
    Lathe {
        at: V3,
        to_local: [V3; 3],
        oval: f64,
        /// The profile, in body units.
        polygon: Vec<V2>,
        /// On the axis, so all of it counts, the hollow of a hat too; a ring
        /// is only the ring.
        filled: bool,
    },
    Ball {
        centre: V3,
        radius: f64,
    },
    /// Spans of a tube: each end and its radius.
    Tube(Vec<(V3, V3, f64, f64)>),
    Shell {
        offset: f64,
        edge: Vec<V2>,
    },
}

/// The hairline at azimuth `az`, without its scallops.
fn hairline(edge: &[V2], az: f64) -> f64 {
    let n = edge.len();
    if n == 1 {
        return edge[0][1];
    }
    let az = (az + 180.0).rem_euclid(360.0) - 180.0;
    // the span round the circle holding `az`
    let i = edge.iter().rposition(|e| e[0] <= az).unwrap_or(n - 1);
    let (a, b) = (edge[i], edge[(i + 1) % n]);
    let span = (b[0] - a[0]).rem_euclid(360.0).max(1e-6);
    let t = ((az - a[0]).rem_euclid(360.0) / span).clamp(0.0, 1.0);
    a[1] + (b[1] - a[1]) * (1.0 - (t * PI).cos()) / 2.0
}

/// The scallops' reach at azimuth `az`, 0 at a valley and 1 at a tip.
fn scallop_at(n: usize, pointed: bool, az: f64) -> f64 {
    let f = ((az + 180.0) / 360.0 * n as f64).rem_euclid(1.0);
    if pointed {
        1.0 - (2.0 * f - 1.0).abs()
    } else {
        (f * PI).sin().powf(0.55)
    }
}

impl Volume {
    fn contains(&self, p: V3, head: &Head) -> bool {
        match self {
            Volume::Lathe { at, to_local, oval, polygon, filled } => {
                let q = apply(to_local, sub(p, *at));
                let (r, h) = (q[0].hypot(q[2] / oval), q[1]);
                if *filled {
                    widest_at(polygon, h).is_some_and(|w| r < w)
                } else {
                    inside(polygon, [r, h])
                }
            }
            Volume::Ball { centre, radius } => length(sub(p, *centre)) < radius - 1e-3,
            Volume::Tube(spans) => spans.iter().any(|(a, b, ra, rb)| {
                let ab = sub(*b, *a);
                let l2 = dot(ab, ab).max(1e-12);
                let t = (dot(sub(p, *a), ab) / l2).clamp(0.0, 1.0);
                length(sub(p, lerp3(*a, *b, t))) < (ra + (rb - ra) * t) - 1e-3
            }),
            Volume::Shell { offset, edge } => {
                let v = sub(p, head.center);
                let l = length(v);
                if l < 1e-9 {
                    return true;
                }
                let d = scale(v, 1.0 / l);
                let pol = d[1].clamp(-1.0, 1.0).acos().to_degrees();
                let az = d[0].atan2(d[2]).to_degrees();
                pol < hairline(edge, az) && l < head.radius(d) + offset - 1e-3
            }
        }
    }
}

/// The profile's widest radius at height `h`, if it reaches that high.
fn widest_at(profile: &[V2], h: f64) -> Option<f64> {
    profile
        .windows(2)
        .filter(|w| (w[0][1] >= h) != (w[1][1] >= h))
        .map(|w| w[0][0] + (w[1][0] - w[0][0]) * (h - w[0][1]) / (w[1][1] - w[0][1]))
        .reduce(f64::max)
}

/// Even-odd point in polygon.
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

/// The part of `poly` inside `[u0, u1] × [v0, v1]`.
fn clip(poly: &[V2], u0: f64, u1: f64, v0: f64, v1: f64) -> Vec<V2> {
    let mut out = poly.to_vec();
    for (axis, bound, keep_above) in [(0, u0, true), (0, u1, false), (1, v0, true), (1, v1, false)] {
        let inside = |p: &V2| if keep_above { p[axis] >= bound } else { p[axis] <= bound };
        let mut next = Vec::with_capacity(out.len() + 4);
        for i in 0..out.len() {
            let (a, b) = (out[i], out[(i + 1) % out.len()]);
            if inside(&a) {
                next.push(a);
            }
            if inside(&a) != inside(&b) {
                let t = (bound - a[axis]) / (b[axis] - a[axis]);
                next.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
            }
        }
        out = next;
        if out.is_empty() {
            break;
        }
    }
    out
}

fn area(poly: &[V2]) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (p, q) = (poly[i], poly[(i + 1) % n]);
            p[0] * q[1] - q[0] * p[1]
        })
        .sum::<f64>()
        / 2.0
}

/// A recipe's solids as one mesh in body units, ready to draw at any pose.
pub(crate) struct Scene {
    points: Vec<V3>,
    faces: Vec<Face>,
    /// Each colour's lit, mid and dark tones.
    paints: Vec<[String; 3]>,
    rings: Vec<Ring>,
    /// Where each step of distance starts in `faces`, nearest the centre
    /// first (and where the last ends).
    steps: Vec<usize>,
    /// The head's centre.
    head: V3,
}

/// The tones of a colour: the recipe's own in the light and darker ones
/// away from it. A near-black one has no darker: it goes lighter in the
/// light instead, so black hair still shows its form.
fn tones(c: &str) -> [String; 3] {
    let h = c.trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).unwrap_or(0);
    let rgb = [(v >> 16) & 255, (v >> 8) & 255, v & 255].map(|c| c as f64);
    let hex = |c: [f64; 3]| {
        format!(
            "#{:02X}{:02X}{:02X}",
            c[0].round().clamp(0.0, 255.0) as u32,
            c[1].round().clamp(0.0, 255.0) as u32,
            c[2].round().clamp(0.0, 255.0) as u32
        )
    };
    let luma = (rgb[0] * 0.3 + rgb[1] * 0.59 + rgb[2] * 0.11) / 255.0;
    if luma < 0.14 {
        let up = |k: f64| rgb.map(|c| c + (255.0 - c) * k);
        return [hex(up(0.2)), hex(up(0.1)), hex(rgb)];
    }
    // a shadow keeps the hue and leans a little toward violet, like the
    // shade a toon colourist picks, rather than going grey
    let down = |k: f64| [rgb[0] * k, rgb[1] * (k - 0.02), rgb[2] * (k + 0.02)];
    [hex(rgb), hex(down(0.86)), hex(down(0.7))]
}

struct Builder<'a> {
    detail: Detail,
    head: &'a Head,
    /// Box units to body units.
    k: f64,
    center: V2,
    points: Vec<V3>,
    faces: Vec<Face>,
    /// Which volume each face belongs to.
    owners: Vec<usize>,
    volumes: Vec<Volume>,
    rings: Vec<Ring>,
    paints: Vec<String>,
}

impl Builder<'_> {
    /// A box point in body units.
    fn body(&self, p: V3) -> V3 {
        [(p[0] - self.center[0]) * self.k, -(p[1] - self.center[1]) * self.k, p[2] * self.k]
    }

    fn paint(&mut self, c: &str) -> usize {
        match self.paints.iter().position(|p| p == c) {
            Some(i) => i,
            None => {
                self.paints.push(c.to_owned());
                self.paints.len() - 1
            }
        }
    }

    /// `g`'s faces, facing away from the volume they enclose. Its chart
    /// closes round, so column `cols` is column 0's neighbour.
    fn grid(&mut self, g: &Grid, paint: usize, marks: &[(usize, &Mark)], owner: usize) {
        let (rows, cols) = (g.rows.len(), g.rows[0].len());
        let base = self.points.len() as u32;
        for r in &g.rows {
            self.points.extend_from_slice(r);
        }
        let at = |i: usize, j: usize| base + (i * cols + j % cols) as u32;
        let quad = |i: usize, j: usize| {
            (g.rows[i][j], g.rows[i][(j + 1) % cols], g.rows[i + 1][(j + 1) % cols], g.rows[i + 1][j])
        };
        // each cell's normal; the area-weighted flux out of the surface
        // decides which way they all point
        let mid = {
            let n = (rows * cols) as f64;
            scale(g.rows.iter().flatten().fold([0.0; 3], |a, p| add(a, *p)), 1.0 / n)
        };
        let mut normals: Vec<V3> = Vec::with_capacity((rows - 1) * cols);
        let mut flux = 0.0;
        for i in 0..rows - 1 {
            for j in 0..cols {
                let (a, b, c, d) = quad(i, j);
                let n = cross(sub(c, a), sub(d, b));
                flux += dot(n, sub(scale(add(add(a, b), add(c, d)), 0.25), mid));
                normals.push(unit(n));
            }
        }
        if flux < 0.0 {
            for n in &mut normals {
                *n = scale(*n, -1.0);
            }
        }
        let normal = |i: usize, j: usize| normals[i * cols + j % cols];
        // a corner's smooth normal: the cells around it on the same side of
        // any crease as this one
        let smooth = |i: usize, j: usize, own: V3| -> V3 {
            let mut sum = [0.0; 3];
            for (r, c) in [(i.wrapping_sub(1), j + cols - 1), (i.wrapping_sub(1), j), (i, j + cols - 1), (i, j)] {
                if r < rows - 1 {
                    let n = normal(r, c);
                    if dot(n, own) > CREASE {
                        sum = add(sum, n);
                    }
                }
            }
            unit(sum)
        };
        for i in 0..rows - 1 {
            for j in 0..cols {
                let own = normal(i, j);
                if own == [0.0; 3] {
                    continue;
                }
                let (a, b, c, d) = quad(i, j);
                let shading = [smooth(i, j, own), smooth(i, j + 1, own), smooth(i + 1, j + 1, own), smooth(i + 1, j, own)];
                let cover = cover(g, i, j, marks);
                self.faces.push(Face {
                    corners: [at(i, j), at(i, j + 1), at(i + 1, j + 1), at(i + 1, j)],
                    normal: own,
                    shading,
                    centre: scale(add(add(a, b), add(c, d)), 0.25),
                    paint: cover.unwrap_or(paint),
                    above: None,
                    fragments: if cover.is_some() { vec![] } else { marks_on(g, i, j, marks, &shading) },
                });
                self.owners.push(owner);
            }
        }
    }

    fn lathe(&mut self, s: &Spec, at: V3, tilt: V3, profile: &[V2], oval: f64, teeth: Option<(usize, f64)>) {
        let m = rotation(tilt);
        let origin = self.body(at);
        let widest = profile.iter().fold(0.0f64, |a, p| a.max(p[0]));
        let mut cols = ((widest * 2.0) as usize).clamp(self.detail.lathe_cols * 2 / 5, self.detail.lathe_cols);
        if let Some((n, _)) = teeth {
            // a whole number of columns per point, a tip and a valley on them
            cols = n * (cols / n).max(2).next_multiple_of(2);
        }
        let profile = &cut_profile(profile, self.detail.lathe_row)[..];
        let mut rows = Vec::with_capacity(profile.len());
        for p in profile {
            rows.push(
                (0..cols)
                    .map(|j| {
                        let a = -PI + TAU * j as f64 / cols as f64;
                        let h = match teeth {
                            None => p[1],
                            Some((n, cut)) if p[1] > 0.0 => p[1] * (1.0 - cut * (1.0 - scallop_at(n, true, a.to_degrees()))),
                            Some(_) => p[1],
                        };
                        let q = apply(&m, [p[0] * a.sin(), h, p[0] * a.cos() * oval]);
                        add(origin, scale(q, self.k))
                    })
                    .collect(),
            );
        }
        // the chart: u round the axis, v by length along the profile
        let mut v = vec![0.0];
        for w in profile.windows(2) {
            v.push(v[v.len() - 1] + (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]));
        }
        let total = v[v.len() - 1].max(1e-9);
        let v: Vec<f64> = v.iter().map(|x| x / total * 100.0).collect();
        let u: Vec<f64> = (0..=cols).map(|j| -180.0 + 360.0 * j as f64 / cols as f64).collect();
        let paint = self.paint(&s.color);
        let marks = self.mark_paints(s);
        let owner = self.volumes.len();
        let first = self.faces.len();
        self.grid(&Grid { rows, u, v }, paint, &marks, owner);
        let widest_ring = profile.iter().fold([0.0, 0.0], |a: V2, p| if p[0] > a[0] { *p } else { a });
        let ring = Ring {
            at: origin,
            to_local: transpose(&m),
            oval,
            height: widest_ring[1] * self.k,
            radius: widest_ring[0] * self.k,
        };
        for f in &mut self.faces[first..] {
            // a face looking down is the ring's own underside, not behind it
            let looks_down = apply(&ring.to_local, f.normal)[1] < -0.5;
            if !looks_down && apply(&ring.to_local, sub(f.centre, origin))[1] > ring.height {
                f.above = Some(self.rings.len());
            }
        }
        self.rings.push(ring);
        self.volumes.push(Volume::Lathe {
            at: origin,
            to_local: transpose(&m),
            oval,
            polygon: profile.iter().map(|p| [p[0] * self.k, p[1] * self.k]).collect(),
            filled: profile[0][0] < 1e-9,
        });
    }

    fn mark_paints<'s>(&mut self, s: &'s Spec) -> Vec<(usize, &'s Mark)> {
        s.marks.iter().map(|m| (self.paint(&m.fill), m)).collect()
    }

    fn shell(&mut self, s: &Spec, offset: f64, edge: &[V2], scallop: Option<(usize, f64, bool)>, lobes: Option<(usize, f64, f64)>) {
        let off = offset * self.k;
        let d = self.detail;
        let cols = match scallop {
            None => d.shell_cols,
            Some((n, _, _)) => n * (d.shell_cols / n).clamp(d.scallop_cols, (d.shell_max_cols / n).max(d.scallop_cols)),
        };
        let line = |az: f64| {
            hairline(edge, az) + scallop.map_or(0.0, |(n, amp, pointed)| amp * scallop_at(n, pointed, az))
        };
        let head = self.head;
        let mut rows: Vec<Vec<V3>> = Vec::with_capacity(d.shell_rows + 1);
        for i in 0..d.shell_rows {
            let t = i as f64 / (d.shell_rows - 1) as f64;
            // rows crowd toward the hairline, where the shape is
            let t = 1.0 - (1.0 - t) * (1.0 - t);
            rows.push(
                (0..cols)
                    .map(|j| {
                        let az = -180.0 + 360.0 * j as f64 / cols as f64;
                        let d = Head::direction(az, line(az) * t);
                        // the edge curls in toward the skull
                        let r = head.radius(d) + off * (1.0 - 0.3 * t.powi(8));
                        add(head.center, scale(d, r))
                    })
                    .collect(),
            );
        }
        // the lip: the edge's thickness, back into the head
        rows.push(
            (0..cols)
                .map(|j| {
                    let az = -180.0 + 360.0 * j as f64 / cols as f64;
                    let d = Head::direction(az, line(az));
                    add(head.center, scale(d, head.radius(d) - 0.01))
                })
                .collect(),
        );
        let v: Vec<f64> = (0..=d.shell_rows).map(|i| i as f64 * 100.0 / d.shell_rows as f64).collect();
        let u: Vec<f64> = (0..=cols).map(|j| -180.0 + 360.0 * j as f64 / cols as f64).collect();
        let paint = self.paint(&s.color);
        let marks = self.mark_paints(s);
        let owner = self.volumes.len();
        self.grid(&Grid { rows, u, v }, paint, &marks, owner);
        self.volumes.push(Volume::Shell {
            offset: off,
            edge: edge.to_vec(),
        });
        if let Some((n, radius, lift)) = lobes {
            let places = lobe_places(head, edge, n, radius * self.k, off + lift * self.k);
            // not an error: a saved design that stops drawing is dropped
            // from the list, and with it the rest of the avatar
            if places.is_empty() {
                log::warn!("a shell's {n} lobes don't fit inside its hairline: drawn without them");
            }
            for (centre, r) in places {
                self.lobe(centre, r, paint);
            }
        }
    }

    fn lobe(&mut self, centre: V3, r: f64, paint: usize) {
        let (lobe_rows, lobe_cols) = (self.detail.lobe_rows, self.detail.lobe_cols);
        let mut rows = Vec::with_capacity(lobe_rows + 1);
        for i in 0..=lobe_rows {
            let p = PI * i as f64 / lobe_rows as f64;
            rows.push(
                (0..lobe_cols)
                    .map(|j| {
                        let a = TAU * j as f64 / lobe_cols as f64;
                        add(centre, [r * p.sin() * a.sin(), r * p.cos(), r * p.sin() * a.cos()])
                    })
                    .collect(),
            );
        }
        let owner = self.volumes.len();
        let v: Vec<f64> = (0..=lobe_rows).map(|i| i as f64).collect();
        let u: Vec<f64> = (0..=lobe_cols).map(|i| i as f64).collect();
        self.grid(&Grid { rows, u, v }, paint, &[], owner);
        self.volumes.push(Volume::Ball { centre, radius: r });
    }

    fn tube(&mut self, s: &Spec, path: &[TubePoint], paint: usize) {
        let (sides, steps) = (self.detail.tube_sides, self.detail.tube_steps);
        // Catmull-Rom through the points, radius along with them
        let n = path.len();
        let at = |i: isize| path[i.clamp(0, n as isize - 1) as usize];
        let mut line: Vec<TubePoint> = Vec::new();
        for i in 0..n - 1 {
            let (p0, p1, p2, p3) = (at(i as isize - 1), at(i as isize), at(i as isize + 1), at(i as isize + 2));
            for k in 0..steps {
                let t = k as f64 / steps as f64;
                let (t2, t3) = (t * t, t * t * t);
                let mut q = [0.0; 4];
                for c in 0..3 {
                    q[c] = 0.5
                        * (2.0 * p1[c]
                            + (p2[c] - p0[c]) * t
                            + (2.0 * p0[c] - 5.0 * p1[c] + 4.0 * p2[c] - p3[c]) * t2
                            + (3.0 * p1[c] - p0[c] - 3.0 * p2[c] + p3[c]) * t3);
                }
                q[3] = p1[3] + (p2[3] - p1[3]) * t;
                line.push(q);
            }
        }
        line.push(path[n - 1]);
        let centres: Vec<V3> = line.iter().map(|q| self.body([q[0], q[1], q[2]])).collect();
        let radii: Vec<f64> = line.iter().map(|q| q[3] * self.k).collect();
        let m = centres.len();
        let tangent = |i: usize| unit(sub(centres[(i + 1).min(m - 1)], centres[i.saturating_sub(1)]));
        // a frame carried along the tube, so it doesn't twist
        let mut normal = {
            let t = tangent(0);
            let up = if t[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
            unit(cross(t, up))
        };
        let ring = |c: V3, r: f64, n: V3, t: V3| -> Vec<V3> {
            let b = cross(t, n);
            (0..sides)
                .map(|j| {
                    let a = TAU * j as f64 / sides as f64;
                    add(c, add(scale(n, r * a.cos()), scale(b, r * a.sin())))
                })
                .collect()
        };
        let mut rows = Vec::with_capacity(m + 2);
        // rounded ends: a point just past each end
        rows.push(vec![sub(centres[0], scale(tangent(0), radii[0] * 0.8)); sides]);
        for i in 0..m {
            let t = tangent(i);
            normal = unit(sub(normal, scale(t, dot(normal, t))));
            rows.push(ring(centres[i], radii[i], normal, t));
        }
        rows.push(vec![add(centres[m - 1], scale(tangent(m - 1), radii[m - 1] * 0.8)); sides]);
        let count = rows.len();
        let v: Vec<f64> = (0..count).map(|i| i as f64 * 100.0 / (count - 1) as f64).collect();
        let u: Vec<f64> = (0..=sides).map(|j| -180.0 + 360.0 * j as f64 / sides as f64).collect();
        let marks = self.mark_paints(s);
        let owner = self.volumes.len();
        self.grid(&Grid { rows, u, v }, paint, &marks, owner);
        let spans = (0..m - 1).map(|i| (centres[i], centres[i + 1], radii[i], radii[i + 1])).collect();
        self.volumes.push(Volume::Tube(spans));
    }
}

/// Whether a loop's box `[u0, v0, u1, v1]` meets a cell.
fn touches(b: &[f64; 4], u0: f64, u1: f64, v0: f64, v1: f64) -> bool {
    b[0] <= u1 && b[2] >= u0 && b[1] <= v1 && b[3] >= v0
}

/// The chart rectangle of cell (`i`, `j`).
fn cell(g: &Grid, i: usize, j: usize) -> (f64, f64, f64, f64) {
    (g.u[j], g.u[j + 1], g.v[i], g.v[i + 1])
}

/// The mark that covers cell (`i`, `j`) whole, if one does: the face just
/// takes its colour.
fn cover(g: &Grid, i: usize, j: usize, marks: &[(usize, &Mark)]) -> Option<usize> {
    let (u0, u1, v0, v1) = cell(g, i, j);
    let full = (u1 - u0) * (v1 - v0);
    marks
        .iter()
        .rev()
        .find(|(_, m)| {
            m.loops.iter().zip(&m.bounds).any(|(l, b)| touches(b, u0, u1, v0, v1) && area(&clip(l, u0, u1, v0, v1)).abs() >= full * 0.999)
        })
        .map(|(p, _)| *p)
}

/// The pieces of `marks` on cell (`i`, `j`), on its surface.
fn marks_on(g: &Grid, i: usize, j: usize, marks: &[(usize, &Mark)], shading: &[V3; 4]) -> Vec<Fragment> {
    let cols = g.rows[0].len();
    let (a, b, c, d) = (g.rows[i][j], g.rows[i][(j + 1) % cols], g.rows[i + 1][(j + 1) % cols], g.rows[i + 1][j]);
    let (u0, u1, v0, v1) = cell(g, i, j);
    let mut out = Vec::new();
    for (paint, m) in marks {
        for (l, bound) in m.loops.iter().zip(&m.bounds) {
            if !touches(bound, u0, u1, v0, v1) {
                continue;
            }
            let cut = clip(l, u0, u1, v0, v1);
            if cut.len() < 3 || area(&cut).abs() < 1e-6 {
                continue;
            }
            let (points, normals) = cut
                .iter()
                .map(|p| {
                    let (s, t) = ((p[0] - u0) / (u1 - u0), (p[1] - v0) / (v1 - v0));
                    (
                        lerp3(lerp3(a, b, s), lerp3(d, c, s), t),
                        lerp3(lerp3(shading[0], shading[1], s), lerp3(shading[3], shading[2], s), t),
                    )
                })
                .unzip();
            out.push(Fragment { points, normals, paint: *paint });
        }
    }
    out
}

/// Where a shell's lobes sit, at most `n`: spread evenly over the part of the
/// head the hairline covers (a spiral lattice, grown until `n` of its points
/// fall inside, or as many as ever do), `lift` off the head, with a little
/// size variety. Empty when none fit.
fn lobe_places(head: &Head, edge: &[V2], n: usize, radius: f64, lift: f64) -> Vec<(V3, f64)> {
    let golden = PI * (3.0 - 5f64.sqrt());
    let reach = head.radius([0.0, 1.0, 0.0]).max(1e-6);
    // a lobe's centre stays back from the hairline by most of its width
    let margin = (radius / reach).atan().to_degrees() * 0.8;
    let mut most = Vec::new();
    for total in n..=n * 12 {
        let mut out = Vec::new();
        for i in 0..total {
            let y = 1.0 - 2.0 * (i as f64 + 0.5) / total as f64;
            let pol = y.acos().to_degrees();
            let az = ((i as f64 * golden).to_degrees() + 180.0).rem_euclid(360.0) - 180.0;
            if pol < hairline(edge, az) - margin {
                let d = Head::direction(az, pol);
                let size = 0.88 + 0.24 * ((i as f64 * 12.9898).sin() * 43758.5453).rem_euclid(1.0);
                out.push((add(head.center, scale(d, head.radius(d) + lift)), radius * size));
            }
        }
        if out.len() >= n {
            out.truncate(n);
            return out;
        }
        if out.len() > most.len() {
            most = out;
        }
    }
    most
}

/// A recipe's solids at each detail.
pub(crate) struct Solids {
    fine: Scene,
    coarse: Scene,
}

impl Solids {
    /// `specs` on `head`, in a frame of `k` body units per box unit around
    /// box point `center`.
    pub(super) fn build(specs: &[Spec], head: &Head, k: f64, center: V2) -> Solids {
        Solids {
            fine: Scene::build(FINE, specs, head, k, center),
            coarse: Scene::build(COARSE, specs, head, k, center),
        }
    }

    /// The scene for a mark `size` px across.
    pub(crate) fn at_size(&self, size: f64) -> &Scene {
        if size < SMALL {
            &self.coarse
        } else {
            &self.fine
        }
    }
}

impl Scene {
    /// The mesh of `specs`, on `head`, in a frame of `k` body units per box
    /// unit around box point `center`.
    fn build(detail: Detail, specs: &[Spec], head: &Head, k: f64, center: V2) -> Scene {
        let mut b = Builder {
            detail,
            head,
            k,
            center,
            points: Vec::new(),
            faces: Vec::new(),
            owners: Vec::new(),
            volumes: Vec::new(),
            rings: Vec::new(),
            paints: Vec::new(),
        };
        for s in specs {
            match &s.kind {
                Kind::Lathe { at, tilt, profile, oval, teeth } => b.lathe(s, *at, *tilt, profile, *oval, *teeth),
                Kind::Shell { offset, edge, scallop, lobes } => b.shell(s, *offset, edge, *scallop, *lobes),
                Kind::Tube { path, branches } => {
                    let paint = b.paint(&s.color);
                    b.tube(s, path, paint);
                    for p in branches {
                        b.tube(s, p, paint);
                    }
                }
            }
        }
        // a face wholly inside the head or another solid never shows
        let Builder { mut points, faces, owners, volumes, rings, paints, .. } = b;
        let hidden = |f: &Face, owner: usize| -> bool {
            let samples = f.corners.map(|c| points[c as usize]);
            let mut all = samples.to_vec();
            all.push(f.centre);
            all.iter().all(|p| head.contains(*p))
                || volumes
                    .iter()
                    .enumerate()
                    .any(|(i, v)| i != owner && all.iter().all(|p| v.contains(*p, head)))
        };
        let keep: Vec<bool> = faces.iter().zip(&owners).map(|(f, o)| !hidden(f, *o)).collect();
        let kept = faces.into_iter().zip(keep).filter_map(|(f, k)| k.then_some(f));
        let mut faces: Vec<Face> = kept.flat_map(|f| cut_at_head(f, &mut points, head)).collect();
        let distance = |f: &Face| length(sub(f.centre, head.center));
        faces.sort_by(|a, b| distance(a).total_cmp(&distance(b)));
        let mut steps = vec![0];
        for i in 1..faces.len() {
            if (distance(&faces[i]) / STEP).floor() != (distance(&faces[i - 1]) / STEP).floor() {
                steps.push(i);
            }
        }
        steps.push(faces.len());
        Scene {
            points,
            faces,
            paints: paints.iter().map(|p| tones(p)).collect(),
            rings,
            steps,
            head: head.center,
        }
    }

    /// The solids at `rot`, `r` px per body unit, centred on `mid`, each
    /// point through `map`: what draws behind the head and what in front.
    /// `bleed` is how far past its edges a face draws, in mapped px, so
    /// that neighbouring faces of different tones leave no hairline.
    pub(crate) fn draw(&self, rot: &Rot, r: f64, mid: V2, map: &dyn Fn(V2) -> V2, bleed: f64) -> (Vec<SolidPaint>, Vec<SolidPaint>) {
        let x_axis = bodies::view(rot, [1.0, 0.0, 0.0]);
        let y_axis = bodies::view(rot, [0.0, 1.0, 0.0]);
        let z_axis = bodies::view(rot, [0.0, 0.0, 1.0]);
        // the rotation's rows, from its columns
        let rows = [
            [x_axis[0], y_axis[0], z_axis[0]],
            [x_axis[1], y_axis[1], z_axis[1]],
            [x_axis[2], y_axis[2], z_axis[2]],
        ];
        let screen = |p: V3| -> V2 { map([mid[0] + r * dot(rows[0], p), mid[1] - r * dot(rows[1], p)]) };
        let projected: Vec<V2> = self.points.iter().map(|p| screen(*p)).collect();
        let plane = dot(rows[2], self.head);
        // the light in body space, so a normal needn't turn to be shaded
        let l = unit(LIGHT);
        let light = [
            l[0] * rows[0][0] + l[1] * rows[1][0] + l[2] * rows[2][0],
            l[0] * rows[0][1] + l[1] * rows[1][1] + l[2] * rows[2][1],
            l[0] * rows[0][2] + l[1] * rows[1][2] + l[2] * rows[2][2],
        ];
        // each face: turned away, or which half it's in
        const GONE: u8 = 0;
        const NEAR: u8 = 1;
        const FAR: u8 = 2;
        let seen: Vec<u8> = self
            .faces
            .iter()
            .map(|f| {
                if dot(rows[2], f.normal) <= 0.0 || f.above.is_some_and(|r| self.rings[r].hides(f.centre, rows[2])) {
                    GONE
                } else if dot(rows[2], f.centre) >= plane {
                    NEAR
                } else {
                    FAR
                }
            })
            .collect();
        let mut group: Vec<((usize, u8), Vec<V2>)> = Vec::new();
        let mut marks: Vec<((usize, u8), Vec<V2>)> = Vec::new();
        // one step of one half onto the end of `out`
        let mut step = |span: &[usize], half: u8, out: &mut Vec<SolidPaint>| {
            for (f, seen) in self.faces[span[0]..span[1]].iter().zip(&seen[span[0]..span[1]]) {
                if *seen != half {
                    continue;
                }
                let ring: Vec<V2> = f.corners.iter().map(|c| projected[*c as usize]).collect();
                toned(f.paint, ring, &f.shading.map(|n| dot(light, n)), &mut group);
                for frag in &f.fragments {
                    let ring: Vec<V2> = frag.points.iter().map(|p| screen(*p)).collect();
                    let lit: Vec<f64> = frag.normals.iter().map(|n| dot(light, *n)).collect();
                    toned(frag.paint, ring, &lit, &mut marks);
                }
            }
            // each colour of the group together, the one the last path has
            // first so it can carry on into it; then the marks over them
            for (is_marks, list) in [(false, &mut group), (true, &mut marks)] {
                let last = out.last().map(|l| l.key);
                list.sort_by_key(|(k, _)| (Some(*k) != last, *k));
                for (key, ring) in list.drain(..) {
                    if out.last().is_none_or(|l| l.key != key) {
                        out.push(SolidPaint {
                            key,
                            fill: self.paints[key.0][key.1 as usize].clone(),
                            loops: Vec::new(),
                        });
                    }
                    if let Some(l) = out.last_mut() {
                        push_loop(&mut l.loops, ring, if is_marks { Some(bleed) } else { None }, bleed);
                    }
                }
            }
        };
        let (mut behind, mut front) = (Vec::new(), Vec::new());
        for span in self.steps.windows(2).rev() {
            step(span, FAR, &mut behind);
        }
        for span in self.steps.windows(2) {
            step(span, NEAR, &mut front);
        }
        (behind, front)
    }
}

/// `ring`'s parts of each tone, into `out` under `paint`, the tones cut
/// along where `lit` (given at each corner) crosses their thresholds.
fn toned(paint: usize, ring: Vec<V2>, lit: &[f64], out: &mut Vec<((usize, u8), Vec<V2>)>) {
    let (lo, hi) = lit.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(*v), b.max(*v)));
    if lo >= LIT {
        out.push(((paint, 0), ring));
    } else if hi < MID {
        out.push(((paint, 2), ring));
    } else if lo >= MID && hi < LIT {
        out.push(((paint, 1), ring));
    } else {
        let (lit_part, rest) = cut(&ring, lit, LIT);
        if lit_part.0.len() >= 3 {
            out.push(((paint, 0), lit_part.0));
        }
        let (mid_part, dark_part) = cut(&rest.0, &rest.1, MID);
        if mid_part.0.len() >= 3 {
            out.push(((paint, 1), mid_part.0));
        }
        if dark_part.0.len() >= 3 {
            out.push(((paint, 2), dark_part.0));
        }
    }
}

/// A ring with a value at each point.
type Valued = (Vec<V2>, Vec<f64>);

/// The parts of a convex `ring` where `value` is at least `at`, and below it.
fn cut(ring: &[V2], value: &[f64], at: f64) -> (Valued, Valued) {
    let (mut above, mut below): (Valued, Valued) = ((vec![], vec![]), (vec![], vec![]));
    let n = ring.len();
    for i in 0..n {
        let j = (i + 1) % n;
        let (a, b, va, vb) = (ring[i], ring[j], value[i], value[j]);
        let side = if va >= at { &mut above } else { &mut below };
        side.0.push(a);
        side.1.push(va);
        if (va >= at) != (vb >= at) {
            let t = (at - va) / (vb - va);
            let p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            above.0.push(p);
            above.1.push(at);
            below.0.push(p);
            below.1.push(at);
        }
    }
    (above, below)
}

/// `ring`, wound one way (so a path of many fills their union) and grown
/// by `bleed`; nothing if it's edge-on. A sliver of a surface grows less, so
/// it doesn't show as a speck; a mark's pieces close up whatever they are.
fn push_loop(loops: &mut Vec<Vec<V2>>, mut ring: Vec<V2>, fixed: Option<f64>, bleed: f64) {
    let a = area(&ring);
    if a.abs() < 0.02 {
        return;
    }
    if a < 0.0 {
        ring.reverse();
    }
    let grow = fixed.unwrap_or(bleed.min(a.abs().sqrt() * 0.5));
    let n = ring.len() as f64;
    let c = ring.iter().fold([0.0, 0.0], |c, p| [c[0] + p[0] / n, c[1] + p[1] / n]);
    for p in &mut ring {
        let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
        let l = dx.hypot(dy).max(1e-6);
        *p = [p[0] + dx / l * grow, p[1] + dy / l * grow];
    }
    loops.push(ring);
}

/// One path of a solid: a fill and the loops it covers, wound one way.
#[derive(Clone)]
pub(crate) struct SolidPaint {
    key: (usize, u8),
    pub fill: String,
    pub loops: Vec<Vec<V2>>,
}

/// Loops as path data, at a tenth of a px: `gc`'s form, written by hand, as
/// a solid has far more points than anything else a mark draws.
pub(crate) fn path_data(loops: &[Vec<V2>]) -> String {
    let mut d = String::with_capacity(loops.iter().map(|l| l.len() * 12 + 2).sum());
    for l in loops {
        for (i, p) in l.iter().enumerate() {
            d.push(if i == 0 { 'M' } else { ' ' });
            tenths(&mut d, p[0]);
            d.push(' ');
            tenths(&mut d, p[1]);
        }
        d.push('Z');
    }
    d
}

fn tenths(out: &mut String, v: f64) {
    let t = (v * 10.0).round() as i64;
    if t < 0 {
        out.push('-');
    }
    let t = t.unsigned_abs();
    let mut digits = [0u8; 20];
    let (mut n, mut whole) = (0, t / 10);
    loop {
        digits[n] = b'0' + (whole % 10) as u8;
        n += 1;
        whole /= 10;
        if whole == 0 {
            break;
        }
    }
    for i in (0..n).rev() {
        out.push(digits[i] as char);
    }
    if !t.is_multiple_of(10) {
        out.push('.');
        out.push((b'0' + (t % 10) as u8) as char);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{custom, shared_tables, uh, Mark, MarkTree, PaintOpts, UhSize};
    use super::*;

    const HEAD: &str = "M50 30 C68 30 78 44 78 61 C78 78 66 91 50 91 C34 91 22 78 22 61 C22 44 32 30 50 30 Z";

    /// A round head wearing `solids`.
    fn recipe(solids: Value) -> String {
        serde_json::json!({"o": HEAD, "c": "#F6D1AE", "l": [["M40 76 L60 76 L60 79 L40 79 Z", "#A8503A", 1.0]], "e": "#111111", "s": solids}).to_string()
    }

    fn brim_hat() -> Value {
        serde_json::json!({"kind": "lathe", "color": "#F2C062", "at": [50, 47, 0], "profile": [
            [0, 31], [14, 29.5], [24, 22], [27, 9], [28, 0], [39, -1.4], [47, -4.8], [47.4, -6], [40, -4.6], [27, -2.2], [0, -2]]})
    }

    fn hair() -> Value {
        serde_json::json!({"kind": "shell", "color": "#151515", "offset": 2.0,
            "edge": [[-180, 125], [-70, 86], [-30, 66], [30, 66], [70, 86], [180, 125]], "scallop": [12, 10, 1]})
    }

    fn afro() -> Value {
        serde_json::json!({"kind": "shell", "color": "#2E5BDB", "offset": 6,
            "edge": [[-180, 125], [-60, 90], [0, 45], [60, 90]], "lobes": [16, 12, 1]})
    }

    fn antler() -> Value {
        serde_json::json!({"kind": "tube", "color": "#9A5B3A", "path": [[26, 30, 0, 4], [12, 18, 2, 3], [6, 4, 2, 1.5]],
            "branches": [[[14, 20, 2, 2.5], [4, 20, 6, 1.2]]]})
    }

    /// The registered body's name and paint.
    fn body(solids: Value) -> (&'static str, std::rc::Rc<custom::Rich>) {
        let name = custom::rich_body(&recipe(solids)).unwrap();
        (name, custom::rich(name).unwrap())
    }

    fn pose(yaw: f64, pitch: f64) -> Rot {
        Rot { yaw, pitch, roll: 0.0 }
    }

    fn bounds(l: &[SolidPaint]) -> [f64; 4] {
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for p in l.iter().flat_map(|l| &l.loops).flatten() {
            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
        }
        b
    }

    fn draw(rich: &custom::Rich, rot: &Rot) -> (Vec<SolidPaint>, Vec<SolidPaint>) {
        rich.solids.as_ref().unwrap().at_size(100.0).draw(rot, 100.0, [200.0, 200.0], &|p| p, 0.5)
    }

    /// Whole-figure box of what's drawn.
    fn whole(d: &(Vec<SolidPaint>, Vec<SolidPaint>)) -> [f64; 4] {
        let (a, b) = (bounds(&d.0), bounds(&d.1));
        [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
    }

    /// A brim is round: as wide at every turn, thin from the side and a
    /// circle from above. It never turns edge-on into a leaf.
    #[test]
    fn a_brim_stays_round_at_every_turn() {
        let (_, rich) = body(serde_json::json!([brim_hat()]));
        let widths: Vec<f64> = (0..16)
            .map(|i| {
                let b = whole(&draw(&rich, &pose(i as f64 * 22.5, 0.0)));
                b[2] - b[0]
            })
            .collect();
        let (lo, hi) = widths.iter().fold((f64::INFINITY, 0.0f64), |(a, b), w| (a.min(*w), b.max(*w)));
        assert!(hi / lo < 1.01, "{widths:?}");
        // from above, as tall as it is wide
        let b = whole(&draw(&rich, &pose(30.0, 89.0)));
        let (w, h) = (b[2] - b[0], b[3] - b[1]);
        assert!((w / h - 1.0).abs() < 0.03, "{w} {h}");
        // from the side, wide and thin next to the crown
        let side = whole(&draw(&rich, &pose(90.0, 0.0)));
        assert!(side[2] - side[0] > (side[3] - side[1]) * 1.5, "{side:?}");
    }

    /// Tilted toward the viewer, the brim's near side passes in front of
    /// the head and its far side behind it.
    #[test]
    fn a_brim_goes_in_front_and_behind() {
        let (_, rich) = body(serde_json::json!([brim_hat()]));
        for yaw in [0.0, 70.0, 180.0, 300.0] {
            let (behind, front) = draw(&rich, &pose(yaw, 35.0));
            assert!(!behind.is_empty() && !front.is_empty());
            // near goes down the screen, far up it
            let (b, f) = (bounds(&behind), bounds(&front));
            assert!(f[3] > b[3] + 20.0, "{yaw}: {f:?} {b:?}");
        }
    }

    /// Seen from below, a brim hides its crown: nothing of the band shows
    /// through, at any turn, and it's back as soon as the view is level.
    #[test]
    fn a_brim_hides_its_crown_from_below() {
        let mut hat = brim_hat();
        hat["marks"] = serde_json::json!([{"path": "M-180 30 L180 30 L180 40 L-180 40 Z", "fill": "#E0393A"}]);
        let (_, rich) = body(serde_json::json!([hat]));
        let band = |rot: &Rot| {
            let (a, b) = draw(&rich, rot);
            a.iter().chain(&b).any(|p| tones("#E0393A").contains(&p.fill))
        };
        for i in 0..16 {
            let yaw = i as f64 * 22.5;
            assert!(band(&pose(yaw, 0.0)), "level at {yaw}");
            assert!(band(&pose(yaw, 40.0)), "above at {yaw}");
            assert!(!band(&pose(yaw, -55.0)), "below at {yaw}");
        }
    }

    /// A plain quad at `corners`, for the cut to work on.
    fn quad(points: &mut Vec<V3>, corners: [V3; 4]) -> Face {
        let at = points.len() as u32;
        points.extend(corners);
        Face {
            corners: [at, at + 1, at + 2, at + 3],
            normal: [0.0, 0.0, 1.0],
            shading: [[0.0, 0.0, 1.0]; 4],
            centre: scale(add(add(corners[0], corners[1]), add(corners[2], corners[3])), 0.25),
            paint: 0,
            above: None,
            fragments: Vec::new(),
        }
    }

    /// How far under the surface a chord of the unit disc head may run here.
    const CHORD: f64 = 0.4;

    fn disc_head() -> Head {
        let rim: Vec<V3> = (0..90)
            .map(|i| {
                let t = TAU * i as f64 / 90.0;
                [t.cos(), t.sin(), 0.0]
            })
            .collect();
        Head::new(&rim, 0.72)
    }

    /// However a face enters the head, what's left of it is outside, and
    /// reaches the surface along the cut.
    #[test]
    fn a_face_entering_the_head_stops_at_its_surface() {
        let head = disc_head();
        let (near, far) = (0.5, 1.5);
        // the same square, moved so one, two or three corners are buried
        for (name, x, y) in [("one", far, far), ("two", far, near), ("three", 0.8, 0.8)] {
            let mut points = Vec::new();
            let c = |dx: f64, dy: f64| [dx, dy, 0.0];
            let (x0, y0) = (x - 1.0, y - 1.0);
            let face = quad(&mut points, [c(x0, y0), c(x, y0), c(x, y), c(x0, y)]);
            let pieces = cut_at_head(face, &mut points, &head);
            assert!(!pieces.is_empty(), "{name}");
            let mut on_surface = 0;
            for p in pieces.iter().flat_map(|f| f.corners) {
                let p = points[p as usize];
                assert!(!head.contains(p), "{name}: {p:?} is inside");
                if length(sub(p, head.center)) < 1.0 + 1e-3 {
                    on_surface += 1;
                }
            }
            assert!(on_surface >= 2, "{name}: {on_surface} corners on the surface");
            for f in &pieces {
                assert!(buried_by(f.centre) < CHORD, "{name}: centre {:?}", f.centre);
            }
        }
    }

    /// Straight out from the centre the surface is the head's depth, however
    /// close to the axis: a point just off it isn't buried a head away.
    #[test]
    fn the_head_ends_at_its_depth_on_its_axis() {
        let head = disc_head();
        for lean in [0.0, 1e-6, 1e-3, 5e-3] {
            let d = unit([lean, 0.0, 1.0]);
            assert!(head.radius(d) < 0.8, "{lean}: {}", head.radius(d));
        }
        assert!(!head.contains([0.001, 0.0, 1.5]), "a point past the depth is outside");
    }

    /// How far a point lies inside the unit disc head.
    fn buried_by(p: V3) -> f64 {
        1.0 - length(p)
    }

    /// Buried corners that alternate leave two separate pieces, with the
    /// head between them: nothing bridges it. An edge between two crossings
    /// is a chord, so it runs a little under a curved surface, no more.
    #[test]
    fn alternate_buried_corners_leave_two_pieces() {
        let head = disc_head();
        let mut points = Vec::new();
        let corners = [[0.95, 0.0, 0.0], [1.2, 1.2, 0.0], [0.0, 0.95, 0.0], [-1.2, -1.2, 0.0]];
        let face = quad(&mut points, corners);
        let pieces = cut_at_head(face, &mut points, &head);
        assert_eq!(pieces.len(), 2);
        for f in &pieces {
            let at = f.corners.map(|c| points[c as usize]);
            assert!(buried_by(f.centre) < CHORD, "centre {:?}", f.centre);
            for i in 0..4 {
                let mid = scale(add(at[i], at[(i + 1) % 4]), 0.5);
                assert!(buried_by(mid) < CHORD, "edge {i} of {at:?}");
            }
        }
    }

    /// A piece of three corners repeats its last, and its centre is still
    /// the middle of the three: the centre decides which half it draws in.
    #[test]
    fn a_cut_triangle_centres_on_its_three_corners() {
        let head = disc_head();
        let mut points = Vec::new();
        let face = quad(&mut points, [[-0.2, -0.2, 0.0], [0.8, -0.2, 0.0], [0.8, 0.8, 0.0], [-0.2, 0.8, 0.0]]);
        let pieces = cut_at_head(face, &mut points, &head);
        assert_eq!(pieces.len(), 1);
        let [a, b, c, d] = pieces[0].corners;
        assert_eq!(c, d, "three corners, the last repeated");
        let middle = scale(add(add(points[a as usize], points[b as usize]), points[c as usize]), 1.0 / 3.0);
        assert!(length(sub(pieces[0].centre, middle)) < 1e-9, "{:?} against {middle:?}", pieces[0].centre);
    }

    /// A face wholly outside comes back as it was.
    #[test]
    fn a_face_outside_the_head_is_left_whole() {
        let head = disc_head();
        let mut points = Vec::new();
        let face = quad(&mut points, [[2.0, 2.0, 0.0], [3.0, 2.0, 0.0], [3.0, 3.0, 0.0], [2.0, 3.0, 0.0]]);
        let corners = face.corners;
        let pieces = cut_at_head(face, &mut points, &head);
        assert_eq!(pieces.len(), 1);
        assert_eq!(pieces[0].corners, corners);
        assert_eq!(points.len(), 4, "no new points");
    }

    /// A dome sunk into the head ends at the surface, not a face past it.
    #[test]
    fn a_solid_sinking_into_the_head_has_no_face_inside_it() {
        let dome = serde_json::json!({"kind": "lathe", "color": "#F2C062", "at": [50, 74, 6], "tilt": [90, 0, 0],
            "oval": 0.72, "profile": [[0, 16], [8, 15], [16, 11], [22, 3], [22, 0], [0, 0]]});
        let (name, rich) = body(serde_json::json!([dome]));
        let scene = rich.solids.as_ref().unwrap().at_size(100.0);
        let base = custom::find(name).unwrap();
        let head = Head::new(&base.loft_points, base.depth);
        let buried = scene.faces.iter().flat_map(|f| f.corners).filter(|c| head.contains(scene.points[*c as usize]));
        assert_eq!(buried.count(), 0, "corners inside the head");
        assert!(!scene.faces.is_empty());
    }

    /// Hair covers the head from the side and the back, where a flat pad
    /// would vanish.
    #[test]
    fn hair_shows_from_the_side_and_back() {
        let (_, rich) = body(serde_json::json!([hair()]));
        let size = |yaw: f64| {
            let (_, front) = draw(&rich, &pose(yaw, 0.0));
            let b = bounds(&front);
            (b[2] - b[0]) * (b[3] - b[1])
        };
        let (front, side, back) = (size(0.0), size(90.0), size(180.0));
        assert!(side > front * 0.6 && back > front, "{front} {side} {back}");
    }

    /// The order against a reference: a ray per pixel through every face
    /// and the head. What the painter leaves on top must be what the ray
    /// meets first, at every pose.
    #[test]
    fn painted_order_matches_ray_casting() {
        let (name, rich) = body(serde_json::json!([hair(), brim_hat(), antler()]));
        let afro_body = body(serde_json::json!([afro()]));
        let base = custom::find(name).unwrap();
        for (rich, base) in [(&rich, base.clone()), (&afro_body.1, custom::find(afro_body.0).unwrap())] {
            let head = Head::new(&base.loft_points, base.depth);
            let scene = rich.solids.as_ref().unwrap().at_size(100.0);
            let (mut wrong, mut covered) = (0, 0);
            for (yaw, pitch) in [(0.0, 0.0), (30.0, 15.0), (75.0, -20.0), (135.0, 40.0), (200.0, -45.0), (290.0, 60.0), (345.0, -70.0)] {
                let rot = pose(yaw, pitch);
                let (behind, front) = scene.draw(&rot, 100.0, [0.0, 0.0], &|p| p, 0.5);
                let m = [bodies::view(&rot, [1.0, 0.0, 0.0]), bodies::view(&rot, [0.0, 1.0, 0.0]), bodies::view(&rot, [0.0, 0.0, 1.0])];
                let turn = |p: V3| add(add(scale(m[0], p[0]), scale(m[1], p[1])), scale(m[2], p[2]));
                let back = |p: V3| [dot(m[0], p), dot(m[1], p), dot(m[2], p)];
                let tris: Vec<([V3; 3], usize)> = scene
                    .faces
                    .iter()
                    .filter(|f| turn(f.normal)[2] > 0.0)
                    .flat_map(|f| {
                        let c = f.corners.map(|c| turn(scene.points[c as usize]));
                        [([c[0], c[1], c[2]], f.paint), ([c[0], c[2], c[3]], f.paint)]
                    })
                    .collect();
                const N: i32 = 44;
                for iy in -N..=N {
                    for ix in -N..=N {
                        let (x, y) = (ix as f64 * 1.5 / N as f64, iy as f64 * 1.5 / N as f64);
                        // the ray's first solid
                        let mut hit: Option<(f64, usize)> = None;
                        for (t, paint) in &tris {
                            if let Some(z) = meets(t, x, y)
                                && hit.is_none_or(|h| z > h.0)
                            {
                                hit = Some((z, *paint));
                            }
                        }
                        // the head: its nearest point on the ray
                        let head_z = (0..=240).rev().map(|k| -1.2 + 2.4 * k as f64 / 240.0).find(|z| head.contains(back([x, y, *z])));
                        let want = match (hit, head_z) {
                            (None, None) => continue,
                            (Some((_, p)), None) => Some(p),
                            (None, Some(_)) => None,
                            (Some((z, p)), Some(h)) => (z > h).then_some(p),
                        };
                        let got = top(&front, x, y)
                            .or_else(|| if head_z.is_some() { Some(usize::MAX) } else { top(&behind, x, y) })
                            .filter(|p| *p != usize::MAX);
                        covered += 1;
                        if got != want {
                            wrong += 1;
                        }
                    }
                }
            }
            let rate = wrong as f64 / covered as f64;
            println!("{wrong} of {covered} pixels differ ({:.2}%)", rate * 100.0);
            assert!(rate < 0.02, "{wrong} of {covered}");
        }
    }

    /// Where the ray down through (`x`, `y`) meets a triangle: its depth.
    fn meets(t: &[V3; 3], x: f64, y: f64) -> Option<f64> {
        let [a, b, c] = t;
        let d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
        if d.abs() < 1e-12 {
            return None;
        }
        let u = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / d;
        let v = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / d;
        let w = 1.0 - u - v;
        (u >= 0.0 && v >= 0.0 && w >= 0.0).then(|| u * a[2] + v * b[2] + w * c[2])
    }

    /// The paint on top at (`x`, `y`) in screen space (y down, 100px a unit).
    fn top(list: &[SolidPaint], x: f64, y: f64) -> Option<usize> {
        let p = [x * 100.0, -y * 100.0];
        list.iter().rev().find(|l| l.loops.iter().any(|r| inside(r, p))).map(|l| l.key.0)
    }

    /// Faces inside the head or a hat are gone, so nothing pokes through;
    /// the root of an antler in a hat leaves the mesh.
    #[test]
    fn hidden_faces_are_dropped() {
        let count = |s: Value| body(s).1.solids.as_ref().unwrap().fine.faces.len();
        let hair_alone = count(serde_json::json!([hair()]));
        let under_hat = count(serde_json::json!([hair(), brim_hat()])) - count(serde_json::json!([brim_hat()]));
        assert!(under_hat < hair_alone, "{under_hat} {hair_alone}");
        let (_, rich) = body(serde_json::json!([brim_hat()]));
        let scene = &rich.solids.as_ref().unwrap().fine;
        // the hat's flat underside, inside the head, isn't there
        let mid = scene.head;
        assert!(scene.faces.iter().all(|f| length(sub(f.centre, mid)) > 0.05));
    }

    /// Marks ride the surface: the band shows all round the hat, the cross
    /// only while its side of the hat faces the viewer.
    #[test]
    fn marks_turn_with_the_surface() {
        let mut hat = brim_hat();
        hat["marks"] = serde_json::json!([
            {"path": "M-180 30 L180 30 L180 40 L-180 40 Z", "fill": "#E0393A"},
            {"path": "M-20 20 L20 20 L20 28 L-20 28 Z", "fill": "#FFFFFF"},
        ]);
        let (_, rich) = body(serde_json::json!([hat]));
        let has = |rot: &Rot, c: &str| {
            let (a, b) = draw(&rich, rot);
            a.iter().chain(&b).any(|p| tones(c).contains(&p.fill))
        };
        for i in 0..8 {
            assert!(has(&pose(i as f64 * 45.0, 10.0), "#E0393A"), "{i}");
        }
        assert!(has(&pose(0.0, 0.0), "#FFFFFF"));
        assert!(!has(&pose(180.0, 0.0), "#FFFFFF"));
    }

    fn tree(mark: &Mark, box_: f64) -> MarkTree {
        let extent = box_ * 0.92 / 0.6;
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
                display_size: Some(box_),
                extent,
            },
            &shared_tables(),
        )
    }

    fn count(t: &MarkTree, attr: &str) -> usize {
        t.attr(attr).is_some() as usize + t.children.iter().map(|c| count(c, attr)).sum::<usize>()
    }

    /// Solids draw through every clip, on both of the renderer's branches,
    /// and blinks and poses still play. Past a side-on turn the mark shows
    /// its back: no eyes, the solids still there.
    #[test]
    fn a_body_with_solids_moves_and_shows_its_back() {
        use super::super::Director;
        let (name, _) = body(serde_json::json!([hair(), brim_hat()]));
        let tab = shared_tables();
        let mut mark = Mark::new(&tab, Some(name), 1.0, false, false, 7);
        let mut dir = Director::new(&mut mark, "thinking", 7, false, false, None);
        for f in 0..180 {
            dir.advance(&mut mark, 1.0 / 30.0);
            mark.advance(1.0 / 30.0);
            if f == 90 {
                dir.set_state_pub(&mut mark, "surprised");
            }
            let t = tree(&mark, 52.48);
            if mark.frame().eyes.is_some() {
                assert!(count(&t, "data-solid") > 5, "frame {f}");
            }
        }
        // round the back
        let mut frame = mark.frame();
        frame.turn = [0.0, 170.0, 0.0];
        frame.morph = None;
        let extent = 52.48 * 0.92 / 0.6;
        let t = uh(
            &frame,
            &PaintOpts { cutout: true, ink_paint: Some("#1884FC".to_owned()), ..Default::default() },
            extent,
            "m",
            &UhSize { display_size: Some(52.48), extent },
            &tab,
        );
        assert_eq!(count(&t, "data-eye"), 0, "the eyes are round the front");
        assert!(count(&t, "data-solid") > 5);
    }

    /// A turn is smooth all the way round: nothing jumps at side-on, where
    /// the kit's pose folds, with the head pitched and rolled too.
    #[test]
    fn a_full_turn_has_no_jump() {
        use super::super::render::turned;
        for (pitch, roll) in [(0.0, 0.0), (12.0, 0.0), (-20.0, 6.0), (35.0, -8.0)] {
            let at = |yaw: f64| {
                let r = turned([pitch, yaw, roll]);
                [bodies::view(&r, [0.0, 0.0, 1.0]), bodies::view(&r, [1.0, 0.0, 0.0]), bodies::view(&r, [0.0, 1.0, 0.0])]
            };
            let mut last = at(0.0);
            let mut yaw = 0.25;
            while yaw <= 720.0 {
                let now = at(yaw);
                let step = (0..3).map(|i| length(sub(now[i], last[i]))).fold(0.0, f64::max);
                // a quarter degree of turn, and the half degree `sd` steps
                // off the edge
                assert!(step < 0.03, "pitch {pitch} roll {roll} yaw {yaw}: {step}");
                last = now;
                yaw += 0.25;
            }
        }
    }

    /// A recipe without solids is as it was: nothing of this module touches
    /// it.
    #[test]
    fn no_solids_no_scene() {
        let plain = serde_json::json!({"o": HEAD, "c": "#F6D1AE", "l": [["M40 76 L60 76 L60 79 L40 79 Z", "#A8503A", 1.0]], "e": "#111111"}).to_string();
        let parts = serde_json::json!({"o": HEAD, "c": "#F6D1AE", "l": [["M40 76 L60 76 L60 79 L40 79 Z", "#A8503A", 1.0]], "e": "#111111",
            "p": [{"o": "M20 20 L80 20 L80 40 L20 40 Z", "c": "#112233", "l": [], "z": 0.1}]}).to_string();
        for r in [plain, parts] {
            let name = custom::rich_body(&r).unwrap();
            assert!(custom::rich(name).unwrap().solids.is_none());
        }
    }

    /// A bad recipe is an error, never a stall.
    #[test]
    fn bad_solids_are_rejected() {
        let lathe = |f: &dyn Fn(&mut Value)| {
            let mut v = brim_hat();
            f(&mut v);
            serde_json::json!([v])
        };
        let shell = |f: &dyn Fn(&mut Value)| {
            let mut v = afro();
            f(&mut v);
            serde_json::json!([v])
        };
        let tube = |f: &dyn Fn(&mut Value)| {
            let mut v = antler();
            f(&mut v);
            serde_json::json!([v])
        };
        let bad = [
            serde_json::json!("x"),
            serde_json::json!([5]),
            serde_json::json!([{"kind": "cube", "color": "#112233"}]),
            lathe(&|v| v["color"] = "red".into()),
            lathe(&|v| v["at"] = serde_json::json!([50, 40])),
            lathe(&|v| v["at"] = serde_json::json!([500, 40, 0])),
            lathe(&|v| v["tilt"] = serde_json::json!([0, 0, 400])),
            lathe(&|v| v["oval"] = 9.into()),
            lathe(&|v| v["teeth"] = serde_json::json!([2.5, 0.5])),
            lathe(&|v| v["profile"] = serde_json::json!([[1, 2]])),
            lathe(&|v| v["profile"] = serde_json::json!([[10, 2], [20, 3], [30, 4]])),
            lathe(&|v| v["profile"] = serde_json::json!(vec![[0, 1]; 41])),
            lathe(&|v| v["marks"] = serde_json::json!([{"path": "M0 0", "fill": "#112233"}])),
            lathe(&|v| v["marks"] = serde_json::json!(vec![serde_json::json!({"path": "M0 0 L9 0 L9 9 Z", "fill": "#112233"}); 9])),
            shell(&|v| v["offset"] = 99.into()),
            shell(&|v| v["edge"] = serde_json::json!([])),
            shell(&|v| v["edge"] = serde_json::json!([[10, 90], [0, 90]])),
            shell(&|v| v["edge"] = serde_json::json!([[0, 1]])),
            shell(&|v| v["scallop"] = serde_json::json!([40, 5, 0])),
            shell(&|v| v["lobes"] = serde_json::json!([33, 9, 0])),
            tube(&|v| v["path"] = serde_json::json!([[1, 2, 3, 4]])),
            tube(&|v| v["path"] = serde_json::json!(vec![[1, 2, 3, 4]; 13])),
            tube(&|v| v["branches"] = serde_json::json!(vec![serde_json::json!([[1, 2, 3, 4], [4, 5, 6, 1]]); 7])),
            serde_json::json!(vec![brim_hat(); 11]),
        ];
        for s in bad {
            assert!(custom::rich_body(&recipe(s.clone())).is_err(), "{s}");
        }
        // inside every limit, too many faces in all
        let tall: Vec<[f64; 2]> = (0..40).map(|i| [if i == 0 || i == 39 { 0.0 } else { 100.0 }, if i % 2 == 0 { -100.0 } else { 100.0 }]).collect();
        let mut v = brim_hat();
        v["profile"] = serde_json::json!(tall);
        let e = custom::rich_body(&recipe(serde_json::json!(vec![v; 10]))).unwrap_err();
        assert!(e.contains("faces"), "{e}");
    }

    /// A short path can retrace one long edge hundreds of times: its length
    /// in characters says nothing about what it costs. Past the caps it's
    /// refused at once, and the most the caps allow still registers quickly.
    #[test]
    fn retraced_marks_are_refused_or_bounded() {
        let retrace = |n: usize| {
            let mut d = String::from("M-180 0");
            for _ in 0..n {
                d += "L180 0L180 100L-180 0";
            }
            d + "Z"
        };
        let hat = |paths: Vec<String>| {
            let mut v = brim_hat();
            let marks: Vec<Value> = paths.into_iter().map(|p| serde_json::json!({"path": p, "fill": "#112233"})).collect();
            v["marks"] = marks.into();
            v
        };
        // under 4000 characters, tens of thousands of points once sampled
        let long = retrace(170);
        assert!(long.len() <= 4000, "{}", long.len());
        let t = std::time::Instant::now();
        let e = custom::rich_body(&recipe(serde_json::json!([hat(vec![long])]))).unwrap_err();
        assert!(e.contains("points, at most 1000"), "{e}");
        assert!(t.elapsed().as_millis() < 500, "{:?}", t.elapsed());
        // two marks each under the cap, too many together
        let e = custom::rich_body(&recipe(serde_json::json!([hat(vec![retrace(2), retrace(2), retrace(2)])]))).unwrap_err();
        assert!(e.contains("points, at most 2000"), "{e}");
        // the most the budget allows: every point against every face
        let mut heavy = hat(vec![retrace(2), retrace(2)]);
        let wide: Vec<[f64; 2]> = (0..40).map(|i| [if i == 0 || i == 39 { 0.0 } else { 50.0 + (i % 2) as f64 }, 40.0 - 2.0 * i as f64]).collect();
        heavy["profile"] = serde_json::json!(wide);
        let t = std::time::Instant::now();
        let (_, rich) = body(serde_json::json!([heavy]));
        let built = t.elapsed();
        draw(&rich, &pose(30.0, 20.0));
        println!("worst marks: built in {built:?}, drawn by {:?}", t.elapsed());
        assert!(built.as_secs() < 5, "{built:?}");
    }

    /// A coordinate far out of the chart never reaches the sampler.
    #[test]
    fn a_huge_mark_coordinate_is_refused() {
        for d in ["M0 0 L1e300 50 L0 90 Z", "M0 0 L9999 50 L0 90 Z", "M0 0 L90 900 L0 90 Z", "M0 0 C1e300 0 1e300 1 0 90 Z", "m0 0 l1e300 50 l0 40 z"] {
            let mut v = brim_hat();
            v["marks"] = serde_json::json!([{"path": d, "fill": "#112233"}]);
            let e = custom::rich_body(&recipe(serde_json::json!([v]))).unwrap_err();
            assert!(e.contains("coordinates must stay within"), "{d}: {e}");
        }
        // the sampler holds on its own, for the outlines that reach it
        assert!(bodies::us("M0 0 L1e300 0 Z", 0.5).len() < 10000);
    }

    /// Lobes come to at most what was asked; when none fit the shell is
    /// still there, with no lobes.
    #[test]
    fn lobes_are_at_most_what_was_asked() {
        let (name, _) = body(serde_json::json!([afro()]));
        let base = custom::find(name).unwrap();
        let head = Head::new(&base.loft_points, base.depth);
        let edge = [[-180.0, 120.0], [0.0, 45.0]];
        for n in [1, 2, 7, 16, 32] {
            let got = lobe_places(&head, &edge, n, 0.2, 0.1).len();
            assert!((1..=n).contains(&got), "{n}: {got}");
        }
        assert!(lobe_places(&head, &[[0.0, 6.0]], 8, 0.9, 0.1).is_empty());
        let mut tight = afro();
        tight["edge"] = serde_json::json!([[0, 6]]);
        tight["lobes"] = serde_json::json!([8, 40, 0]);
        let plain = |lobes: bool| {
            let mut v = tight.clone();
            if !lobes {
                v.as_object_mut().unwrap().remove("lobes");
            }
            body(serde_json::json!([v])).1.solids.as_ref().unwrap().fine.faces.len()
        };
        assert_eq!(plain(true), plain(false));
    }

    /// `"s": []` is a recipe with no solids: the same body, not a figure
    /// refitted around nothing.
    #[test]
    fn an_empty_list_is_no_solids() {
        let without = serde_json::json!({"o": HEAD, "c": "#F6D1AE", "l": [["M40 76 L60 76 L60 79 L40 79 Z", "#A8503A", 1.0]], "e": "#111111"});
        let mut with = without.clone();
        with["s"] = serde_json::json!([]);
        let (a, b) = (custom::rich_body(&without.to_string()).unwrap(), custom::rich_body(&with.to_string()).unwrap());
        let (a, b) = (custom::find(a).unwrap(), custom::find(b).unwrap());
        assert!(std::rc::Rc::ptr_eq(&a, &b), "the plain body of the outline, shared");
        assert!(custom::rich(custom::rich_body(&with.to_string()).unwrap()).unwrap().solids.is_none());
    }

    /// The heaviest allowed recipe registers and draws in bounded time.
    #[test]
    fn the_heaviest_recipe_stays_bounded() {
        let mut lobes = afro();
        lobes["lobes"] = serde_json::json!([32, 12, 1]);
        let list = serde_json::json!([lobes, lobes]);
        let t = std::time::Instant::now();
        let (_, rich) = body(list);
        let d = draw(&rich, &pose(30.0, 20.0));
        assert!(!d.1.is_empty());
        assert!(t.elapsed().as_secs() < 20, "{:?}", t.elapsed());
    }

    #[test]
    fn path_data_writes_tenths() {
        assert_eq!(path_data(&[vec![[1.0, 2.25], [-0.04, 10.5], [-3.15, 0.0]]]), "M1 2.3 0 10.5 -3.2 0Z");
    }
}
