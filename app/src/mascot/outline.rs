//! The body shapes. Each is generated once at run time: a dense closed
//! curve from a small formula (a polar profile, a parametric curve or a
//! rounded polygon), smoothed, centered, scaled to the unit square and
//! resampled to `OUTLINE_POINTS` evenly spaced points starting straight
//! above the center and running clockwise. The even spacing and the
//! common start let any two outlines blend point by point (a shape change
//! morphs).

use std::f32::consts::{PI, TAU};
use std::sync::OnceLock;

/// Points per outline.
pub(crate) const OUTLINE_POINTS: usize = 72;

/// A closed outline in body units: x right, y down, the body inside
/// `[-1, 1]` on both axes.
pub(crate) type Outline = [[f32; 2]; OUTLINE_POINTS];

/// The picker's shapes, in picker order. These names are what a bot saves
/// (`shape`) and what the server's avatar generator picks from.
pub(crate) const BOT_SHAPES: [&str; 13] = [
    "blob", "cloud", "square", "sparkle", "clover", "heart", "flower", "teardrop", "tablet",
    "wedge", "house", "star", "hex",
];

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Shape {
    Blob,
    Cloud,
    Square,
    Sparkle,
    Clover,
    Heart,
    Flower,
    Teardrop,
    Tablet,
    Wedge,
    House,
    Star,
    Hex,
}

impl Shape {
    /// Every shape, in `BOT_SHAPES` order.
    pub(crate) const ALL: [Shape; 13] = [
        Shape::Blob,
        Shape::Cloud,
        Shape::Square,
        Shape::Sparkle,
        Shape::Clover,
        Shape::Heart,
        Shape::Flower,
        Shape::Teardrop,
        Shape::Tablet,
        Shape::Wedge,
        Shape::House,
        Shape::Star,
        Shape::Hex,
    ];

    /// The shape a saved name means; anything unknown (or empty) is the
    /// default `blob`.
    pub(crate) fn from_name(name: &str) -> Shape {
        let name = name.trim();
        BOT_SHAPES
            .iter()
            .position(|known| known.eq_ignore_ascii_case(name))
            .map_or(Shape::Blob, |index| Shape::ALL[index])
    }

    #[cfg(test)]
    pub(crate) fn name(self) -> &'static str {
        BOT_SHAPES[self as usize]
    }

    /// Where the face sits on this body, in body units. The eyes sit up
    /// and to the right of it (the brand pose), so on narrow or pointed
    /// bodies it moves down and left to keep the pair on the solid middle.
    /// How far the eyes spread around it is fitted at run time
    /// (`figure::face`).
    pub(crate) fn face_center(self) -> [f32; 2] {
        match self {
            Shape::Cloud => [-0.05, 0.12],
            Shape::Sparkle => [-0.1, 0.12],
            Shape::Heart => [-0.1, 0.05],
            Shape::Flower => [-0.05, 0.05],
            Shape::Teardrop => [-0.02, 0.28],
            Shape::Tablet => [-0.06, 0.05],
            Shape::Wedge => [0.0, 0.38],
            Shape::House => [-0.05, 0.25],
            Shape::Star => [-0.08, 0.18],
            Shape::Hex => [-0.04, 0.04],
            _ => [0.0, 0.0],
        }
    }
}

/// The outline of `shape`, built on first use and kept for the process.
pub(crate) fn outline(shape: Shape) -> &'static Outline {
    static OUTLINES: OnceLock<Vec<Outline>> = OnceLock::new();
    &OUTLINES.get_or_init(|| Shape::ALL.iter().map(|s| build(*s)).collect())[shape as usize]
}

/// Dense samples of the raw curve before smoothing.
const DENSE: usize = 360;

fn build(shape: Shape) -> Outline {
    let mut dense = match shape {
        Shape::Blob => polar(|a| 1.0 + 0.025 * (2.0 * a).cos()),
        Shape::Cloud => polar(|a| {
            // a wide oval with bumps across its top and a calmer base
            let oval = 1.0 / ((a.cos() / 1.18).powi(2) + (a.sin() / 0.82).powi(2)).sqrt();
            let top = (0.5 - 0.5 * a.sin()).powf(1.5);
            oval * (1.0 + 0.075 * top * (7.0 * a + 0.4).cos())
        }),
        Shape::Square => superellipse(1.0, 0.96, 4.5),
        Shape::Sparkle => polar(|a| {
            let tip = (0.5 + 0.5 * (4.0 * a).cos()).powf(2.6);
            0.46 + 0.54 * tip
        }),
        Shape::Clover => polar(|a| 0.66 + 0.34 * (2.0 * (a - PI / 4.0)).cos().abs().powf(0.55)),
        Shape::Heart => parametric(|t| {
            let x = 16.0 * t.sin().powi(3);
            let y = 13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos();
            [x, -y]
        }),
        Shape::Flower => polar(|a| 0.74 + 0.26 * (3.0 * a).cos().abs().powf(0.7)),
        Shape::Teardrop => parametric(|t| {
            let (x, y) = (t.sin() * (t / 2.0).sin(), -t.cos());
            let lean: f32 = -0.38;
            [x * lean.cos() - y * lean.sin(), x * lean.sin() + y * lean.cos()]
        }),
        Shape::Tablet => superellipse(0.76, 1.0, 3.4),
        Shape::Wedge => polygon(&[[-0.95, 0.8], [0.95, 0.8], [0.45, -0.95]]),
        Shape::House => polygon(&[[-0.85, 0.92], [0.85, 0.92], [0.85, -0.12], [0.0, -0.95], [-0.85, -0.12]]),
        Shape::Star => polar(|a| {
            let tip = (0.5 + 0.5 * (5.0 * (a + PI / 2.0)).cos()).powf(1.7);
            0.5 + 0.5 * tip
        }),
        Shape::Hex => {
            let corners: Vec<[f32; 2]> = (0..6)
                .map(|k| {
                    let a = TAU * k as f32 / 6.0 - PI / 2.0;
                    [a.cos(), a.sin()]
                })
                .collect();
            polygon(&corners)
        }
    };
    let passes = match shape {
        Shape::Wedge | Shape::House | Shape::Hex => 160,
        Shape::Square | Shape::Tablet | Shape::Blob => 2,
        _ => 10,
    };
    smooth(&mut dense, passes);
    normalize(&mut dense);
    resample(&dense)
}

/// A star-shaped curve from its radius at each angle (screen angles: 0 is
/// right, a quarter turn is down).
fn polar(radius: impl Fn(f32) -> f32) -> Vec<[f32; 2]> {
    (0..DENSE)
        .map(|i| {
            let a = TAU * i as f32 / DENSE as f32;
            let r = radius(a);
            [r * a.cos(), r * a.sin()]
        })
        .collect()
}

/// A closed curve from a point at each parameter in `[0, 2π)`.
fn parametric(point: impl Fn(f32) -> [f32; 2]) -> Vec<[f32; 2]> {
    (0..DENSE).map(|i| point(TAU * i as f32 / DENSE as f32)).collect()
}

/// `|x/w|^n + |y/h|^n = 1`: a rounded rectangle whose corners soften as
/// `n` drops.
fn superellipse(half_width: f32, half_height: f32, n: f32) -> Vec<[f32; 2]> {
    polar(|a| {
        let (c, s) = (a.cos().abs() / half_width, a.sin().abs() / half_height);
        (c.powf(n) + s.powf(n)).powf(-1.0 / n)
    })
}

/// A polygon's edges, sampled evenly; `smooth` rounds its corners.
fn polygon(corners: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let lengths: Vec<f32> = (0..corners.len())
        .map(|i| distance(corners[i], corners[(i + 1) % corners.len()]))
        .collect();
    let perimeter: f32 = lengths.iter().sum();
    let mut points = Vec::with_capacity(DENSE);
    for (i, length) in lengths.iter().enumerate() {
        let (from, to) = (corners[i], corners[(i + 1) % corners.len()]);
        let count = ((length / perimeter) * DENSE as f32).round().max(1.0) as usize;
        for k in 0..count {
            let f = k as f32 / count as f32;
            points.push([from[0] + (to[0] - from[0]) * f, from[1] + (to[1] - from[1]) * f]);
        }
    }
    points
}

/// Repeated neighbour averaging on the closed curve: rounds corners and
/// creases, more with every pass.
fn smooth(points: &mut [[f32; 2]], passes: usize) {
    let n = points.len();
    let mut scratch = points.to_vec();
    for _ in 0..passes {
        for i in 0..n {
            let (prev, here, next) = (points[(i + n - 1) % n], points[i], points[(i + 1) % n]);
            scratch[i] = [
                0.25 * prev[0] + 0.5 * here[0] + 0.25 * next[0],
                0.25 * prev[1] + 0.5 * here[1] + 0.25 * next[1],
            ];
        }
        points.copy_from_slice(&scratch);
    }
}

/// Centers the curve's bounding box on the origin, scales its larger half
/// extent to 1 and makes it run clockwise on screen.
fn normalize(points: &mut [[f32; 2]]) {
    let (mut low, mut high) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in points.iter() {
        for axis in 0..2 {
            low[axis] = low[axis].min(p[axis]);
            high[axis] = high[axis].max(p[axis]);
        }
    }
    let center = [(low[0] + high[0]) / 2.0, (low[1] + high[1]) / 2.0];
    let half = ((high[0] - low[0]).max(high[1] - low[1]) / 2.0).max(1e-6);
    for p in points.iter_mut() {
        *p = [(p[0] - center[0]) / half, (p[1] - center[1]) / half];
    }
    if signed_area(points) < 0.0 {
        points.reverse();
    }
}

/// Twice the signed area (positive: clockwise with y down).
pub(crate) fn signed_area(points: &[[f32; 2]]) -> f32 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

/// `OUTLINE_POINTS` points evenly spaced along the curve, the first where
/// the curve crosses straight up from the center.
fn resample(points: &[[f32; 2]]) -> Outline {
    let n = points.len();
    // start: the point nearest the upward ray, among those above center
    let start = (0..n)
        .filter(|&i| points[i][1] < 0.0)
        .min_by(|&a, &b| points[a][0].abs().total_cmp(&points[b][0].abs()))
        .unwrap_or(0);
    let ordered: Vec<[f32; 2]> = (0..n).map(|k| points[(start + k) % n]).collect();
    let mut cumulative = Vec::with_capacity(n + 1);
    cumulative.push(0.0f32);
    for i in 0..n {
        let step = distance(ordered[i], ordered[(i + 1) % n]);
        cumulative.push(cumulative[i] + step);
    }
    let total = cumulative[n];
    let mut out = [[0.0f32; 2]; OUTLINE_POINTS];
    let mut segment = 0;
    for (k, slot) in out.iter_mut().enumerate() {
        let target = total * k as f32 / OUTLINE_POINTS as f32;
        while segment + 1 < n && cumulative[segment + 1] < target {
            segment += 1;
        }
        let span = (cumulative[segment + 1] - cumulative[segment]).max(1e-9);
        let f = (target - cumulative[segment]) / span;
        let (a, b) = (ordered[segment], ordered[(segment + 1) % n]);
        *slot = [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
    }
    out
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Whether `point` is inside the closed polygon `points` (even-odd).
pub(crate) fn contains(points: &[[f32; 2]], point: [f32; 2]) -> bool {
    let n = points.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (points[i], points[(i + n - 1) % n]);
        if (a[1] > point[1]) != (b[1] > point[1]) {
            let cross_x = a[0] + (point[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
            if point[0] < cross_x {
                inside = !inside;
            }
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_unknown_is_blob() {
        for (i, name) in BOT_SHAPES.iter().enumerate() {
            assert_eq!(Shape::from_name(name), Shape::ALL[i]);
            assert_eq!(Shape::ALL[i].name(), *name);
        }
        assert_eq!(Shape::from_name(" Cloud "), Shape::Cloud);
        assert_eq!(Shape::from_name(""), Shape::Blob);
        assert_eq!(Shape::from_name("dodecahedron"), Shape::Blob);
    }

    #[test]
    fn persisted_shape_names_are_unchanged() {
        // the server's avatar generator and existing bots use these names
        assert_eq!(
            BOT_SHAPES.join("|"),
            "blob|cloud|square|sparkle|clover|heart|flower|teardrop|tablet|wedge|house|star|hex"
        );
    }

    #[test]
    fn outlines_fit_the_unit_square_and_run_clockwise() {
        for shape in Shape::ALL {
            let points = outline(shape);
            let reach = points.iter().fold(0.0f32, |m, p| m.max(p[0].abs()).max(p[1].abs()));
            assert!((reach - 1.0).abs() < 0.03, "{shape:?} reach {reach}");
            assert!(signed_area(points) > 0.5, "{shape:?} is clockwise and solid");
            assert!(points[0][1] < 0.0 && points[0][0].abs() < 0.2, "{shape:?} starts on top");
            assert!(contains(points, shape.face_center()), "{shape:?} face inside");
        }
    }

    #[test]
    fn outlines_are_evenly_spaced() {
        for shape in Shape::ALL {
            let points = outline(shape);
            let steps: Vec<f32> = (0..OUTLINE_POINTS)
                .map(|i| distance(points[i], points[(i + 1) % OUTLINE_POINTS]))
                .collect();
            let mean = steps.iter().sum::<f32>() / steps.len() as f32;
            for s in steps {
                assert!((s - mean).abs() < mean * 0.35, "{shape:?}: {s} vs {mean}");
            }
        }
    }

    #[test]
    fn shapes_differ() {
        for (i, a) in Shape::ALL.iter().enumerate() {
            for b in &Shape::ALL[i + 1..] {
                let (pa, pb) = (outline(*a), outline(*b));
                let gap: f32 = (0..OUTLINE_POINTS).map(|k| distance(pa[k], pb[k])).sum();
                assert!(gap > 1.0, "{a:?} and {b:?} look alike");
            }
        }
    }
}
