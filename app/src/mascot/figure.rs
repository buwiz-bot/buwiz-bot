//! One drawable frame: a `Pose` applied to a body outline gives the body
//! polygon and the two eye polygons, which `paint` fills as one even-odd
//! path (the eyes are holes in the ink). Everything is fixed-size arrays;
//! building a frame allocates nothing.

use nsh_core::canvas::FillRule;
use nsh_core::prelude::*;

use std::sync::OnceLock;

use super::outline::{self, Outline, Shape, OUTLINE_POINTS};

/// Points per eye polygon.
pub(crate) const EYE_POINTS: usize = 20;

pub(crate) type EyePolygon = [[f32; 2]; EYE_POINTS];

/// Where the eyes look when nothing points them anywhere: up and to the
/// right, the brand mascot's pose. x right, y up, both in `[-1, 1]`.
pub(crate) const REST_LOOK: [f32; 2] = [0.55, 0.5];

/// How far a full look moves the eyes, in body units (before the face's
/// spread).
const EYE_REACH: f32 = 0.22;

/// An eye: where it sits at a centered look (face units, y down), the
/// half length between its round ends, its radius and its slant.
struct EyeSpec {
    base: [f32; 2],
    half_length: f32,
    radius: f32,
    slant: f32,
}

/// The nearer eye (larger) and the farther one, slanted like the brand
/// mascot's: tall pills leaning left at the top.
const EYES: [EyeSpec; 2] = [
    EyeSpec { base: [-0.02, -0.22], half_length: 0.12, radius: 0.135, slant: -0.6 },
    EyeSpec { base: [0.45, -0.43], half_length: 0.1, radius: 0.11, slant: -0.68 },
];

/// The body and eye settings of one frame. Lengths are in body radii.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct Pose {
    /// Body offset (x right, y down).
    pub(crate) shift: [f32; 2],
    /// Height over width, area kept, anchored at the base: above 1 is
    /// stretched tall, below 1 squashed.
    pub(crate) stretch: f32,
    /// Rotation about the center, radians clockwise.
    pub(crate) lean: f32,
    /// Uniform scale.
    pub(crate) swell: f32,
    /// Radial wobble of the outline, and its phase.
    pub(crate) ripple: f32,
    pub(crate) ripple_phase: f32,
    /// Look direction: x right, y up, inside the unit disk.
    pub(crate) look: [f32; 2],
    /// Eye openness: 0 shut, 1 open (a little more is wide).
    pub(crate) eye_open: f32,
    /// Eye scale.
    pub(crate) eye_size: f32,
    /// Extra eye slant, radians: positive leans the eyes' tops toward
    /// each other (worried), negative apart (cross).
    pub(crate) eye_tilt: f32,
}

impl Pose {
    pub(crate) const REST: Pose = Pose {
        shift: [0.0, 0.0],
        stretch: 1.0,
        lean: 0.0,
        swell: 1.0,
        ripple: 0.0,
        ripple_phase: 0.0,
        look: REST_LOOK,
        eye_open: 1.0,
        eye_size: 1.0,
        eye_tilt: 0.0,
    };

    /// `self` moved `amount` of the way to `other`.
    pub(crate) fn mix(&self, other: &Pose, amount: f32) -> Pose {
        let k = amount.clamp(0.0, 1.0);
        let m = |a: f32, b: f32| a + (b - a) * k;
        Pose {
            shift: [m(self.shift[0], other.shift[0]), m(self.shift[1], other.shift[1])],
            stretch: m(self.stretch, other.stretch),
            lean: m(self.lean, other.lean),
            swell: m(self.swell, other.swell),
            ripple: m(self.ripple, other.ripple),
            ripple_phase: m(self.ripple_phase, other.ripple_phase),
            look: [m(self.look[0], other.look[0]), m(self.look[1], other.look[1])],
            eye_open: m(self.eye_open, other.eye_open),
            eye_size: m(self.eye_size, other.eye_size),
            eye_tilt: m(self.eye_tilt, other.eye_tilt),
        }
    }
}

/// A look direction pulled into the unit disk (finite, at most length 1).
pub(crate) fn clamp_look(look: [f32; 2]) -> [f32; 2] {
    let [x, y] = look.map(|v| if v.is_finite() { v } else { 0.0 });
    let length = (x * x + y * y).sqrt();
    if length > 1.0 { [x / length, y / length] } else { [x, y] }
}

/// Where the eyes sit on a body: around `center`, spread (and sized) by
/// `spread`, both in body units.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct Face {
    pub(crate) center: [f32; 2],
    pub(crate) spread: f32,
}

/// The largest eyes a pose may ask for: the face is fitted so these stay
/// on the body.
const FIT_EYE_SIZE: f32 = 1.2;
const FIT_EYE_OPEN: f32 = 1.25;
/// The slants a pose may ask for: tops a little apart (cross) to a little
/// together (worried); tall eyes slanted further would touch.
const FIT_TILTS: [f32; 2] = [-0.2, 0.15];
/// The eyes keep this much body around them (a fraction of their offset
/// from the face center).
const FIT_MARGIN: f32 = 1.06;

/// `shape`'s face: its center, and the widest spread at which the
/// biggest eyes, looking any way and slanted either way, stay inside the
/// body and apart. Fitted on first use (a bisection) and kept.
pub(crate) fn face(shape: Shape) -> Face {
    static FACES: [OnceLock<Face>; 13] = [const { OnceLock::new() }; 13];
    *FACES[shape as usize].get_or_init(|| fit_face(shape))
}

fn fit_face(shape: Shape) -> Face {
    let body = outline::outline(shape);
    let center = shape.face_center();
    let fits = |spread: f32| {
        let face = Face { center, spread };
        (0..16).all(|step| {
            let a = std::f32::consts::TAU * step as f32 / 16.0;
            FIT_TILTS.iter().all(|tilt| {
                let pose = Pose {
                    look: [a.cos(), a.sin()],
                    eye_size: FIT_EYE_SIZE,
                    eye_open: FIT_EYE_OPEN,
                    eye_tilt: *tilt,
                    ..Pose::REST
                };
                let [near, far] = eye_polygons(face, &pose);
                let on_body = |eye: &EyePolygon| {
                    eye.iter().all(|p| {
                        let out = [center[0] + (p[0] - center[0]) * FIT_MARGIN, center[1] + (p[1] - center[1]) * FIT_MARGIN];
                        outline::contains(body, out)
                    })
                };
                on_body(&near)
                    && on_body(&far)
                    && near.iter().all(|p| !outline::contains(&far, *p))
                    && far.iter().all(|p| !outline::contains(&near, *p))
            })
        })
    };
    let (mut low, mut high) = (0.2f32, 1.0f32);
    if fits(high) {
        return Face { center, spread: high };
    }
    for _ in 0..12 {
        let mid = (low + high) / 2.0;
        if fits(mid) { low = mid } else { high = mid }
    }
    Face { center, spread: low }
}

/// Both eye polygons for `pose` on `face`, before the body transform.
fn eye_polygons(face: Face, pose: &Pose) -> [EyePolygon; 2] {
    let look = clamp_look(pose.look);
    // eyes shrink slower than the face spreads, so small faces keep
    // readable eyes
    let eye_scale = face.spread.powf(0.75) * pose.eye_size;
    let openness = pose.eye_open.clamp(0.06, FIT_EYE_OPEN);
    [0, 1].map(|which| {
        let spec = &EYES[which];
        let center = [
            face.center[0] + face.spread * (spec.base[0] + look[0] * EYE_REACH),
            face.center[1] + face.spread * (spec.base[1] - look[1] * EYE_REACH),
        ];
        // mirrored per side: a positive tilt leans the tops together
        let tilt = if which == 0 { pose.eye_tilt } else { -pose.eye_tilt };
        let slant = spec.slant + tilt + 0.12 * look[0];
        let (sin, cos) = slant.sin_cos();
        pill(spec.half_length * eye_scale, spec.radius * eye_scale).map(|[x, y]| {
            let y = y * openness;
            [center[0] + x * cos - y * sin, center[1] + x * sin + y * cos]
        })
    })
}

/// The body polygon and both eye polygons, in body units.
#[derive(Clone, Debug)]
pub(crate) struct Figure {
    pub(crate) body: Outline,
    pub(crate) eyes: [EyePolygon; 2],
}

/// The shape a frame draws: one outline, or two being morphed.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct Body {
    pub(crate) from: Shape,
    pub(crate) to: Shape,
    /// 0 is `from`, 1 is `to`.
    pub(crate) blend: f32,
}

impl Body {
    pub(crate) fn of(shape: Shape) -> Body {
        Body { from: shape, to: shape, blend: 1.0 }
    }

    fn face(&self) -> Face {
        let (a, b) = (face(self.from), face(self.to));
        let k = self.blend.clamp(0.0, 1.0);
        Face {
            center: [
                a.center[0] + (b.center[0] - a.center[0]) * k,
                a.center[1] + (b.center[1] - a.center[1]) * k,
            ],
            spread: a.spread + (b.spread - a.spread) * k,
        }
    }
}

impl Figure {
    pub(crate) fn build(body: Body, pose: &Pose) -> Figure {
        let (from, to) = (outline::outline(body.from), outline::outline(body.to));
        let k = body.blend.clamp(0.0, 1.0);
        let place = Placement::new(pose);
        let mut points = [[0.0f32; 2]; OUTLINE_POINTS];
        for (i, slot) in points.iter_mut().enumerate() {
            let (a, b) = (from[i], to[i]);
            let mut p = if k >= 1.0 {
                b
            } else if k <= 0.0 {
                a
            } else {
                [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k]
            };
            if pose.ripple != 0.0 {
                let turn = std::f32::consts::TAU * i as f32 / OUTLINE_POINTS as f32;
                let scale = 1.0 + pose.ripple * (3.0 * turn + pose.ripple_phase).sin();
                p = [p[0] * scale, p[1] * scale];
            }
            *slot = place.apply(p);
        }
        let eyes = eye_polygons(body.face(), pose).map(|eye| eye.map(|p| place.apply(p)));
        Figure { body: points, eyes }
    }

    /// Fills the figure in `ink`, its center at (`center_x`, `center_y`)
    /// and a body radius of `radius` points.
    pub(crate) fn paint(&self, p: Painter, center_x: f32, center_y: f32, radius: f32, ink: Hsla) -> Painter {
        let to_px = |q: [f32; 2]| [center_x + q[0] * radius, center_y + q[1] * radius];
        let mut p = p.begin_path();
        p = smooth_loop(p, &self.body, to_px);
        for eye in &self.eyes {
            p = smooth_loop(p, eye, to_px);
        }
        p.fill_color(ink).fill_with(FillRule::EvenOdd)
    }
}

/// A closed curve through the midpoints of `points`' edges, each point
/// the control of a quadratic piece: smooth, and as cheap as the polygon.
fn smooth_loop(mut p: Painter, points: &[[f32; 2]], to_px: impl Fn([f32; 2]) -> [f32; 2]) -> Painter {
    let n = points.len();
    let mid = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let start = to_px(mid(points[n - 1], points[0]));
    p = p.move_to(start[0], start[1]);
    for i in 0..n {
        let control = to_px(points[i]);
        let end = to_px(mid(points[i], points[(i + 1) % n]));
        p = p.quadratic_curve_to(
            point(px(control[0]), px(control[1])),
            point(px(end[0]), px(end[1])),
        );
    }
    p.close_path()
}

/// An upright pill (two half circles joined by straight sides) centered on
/// the origin, as `EYE_POINTS` points.
fn pill(half_length: f32, radius: f32) -> EyePolygon {
    let half = EYE_POINTS / 2;
    let mut out = [[0.0f32; 2]; EYE_POINTS];
    for (k, slot) in out.iter_mut().enumerate() {
        // top cap sweeps right→left over the top, bottom cap left→right
        let (end, local) = if k < half { (-half_length, k) } else { (half_length, k - half) };
        let a = std::f32::consts::PI * local as f32 / (half - 1) as f32;
        *slot = if k < half {
            [radius * a.cos(), end - radius * a.sin()]
        } else {
            [-radius * a.cos(), end + radius * a.sin()]
        };
    }
    out
}

/// The body transform shared by the outline and the eyes.
struct Placement {
    widen: f32,
    stretch: f32,
    swell: f32,
    sin: f32,
    cos: f32,
    shift: [f32; 2],
}

impl Placement {
    fn new(pose: &Pose) -> Placement {
        let stretch = pose.stretch.clamp(0.5, 1.6);
        let (sin, cos) = pose.lean.sin_cos();
        Placement {
            widen: 1.0 / stretch.sqrt(),
            stretch: stretch.sqrt(),
            swell: pose.swell,
            sin,
            cos,
            shift: pose.shift,
        }
    }

    fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        // stretch about the base so a squash sits down rather than shrinks
        let x = p[0] * self.widen;
        let y = 1.0 + (p[1] - 1.0) * self.stretch;
        let (x, y) = (x * self.swell, y * self.swell);
        [
            x * self.cos - y * self.sin + self.shift[0],
            x * self.sin + y * self.cos + self.shift[1],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mascot::outline::contains;

    fn eyes_inside(shape: Shape, pose: &Pose) -> bool {
        let figure = Figure::build(Body::of(shape), pose);
        figure.eyes.iter().all(|eye| eye.iter().all(|p| contains(&figure.body, *p)))
    }

    #[test]
    fn eyes_stay_on_the_body_for_every_look() {
        for shape in Shape::ALL {
            for step in 0..16 {
                let a = std::f32::consts::TAU * step as f32 / 16.0;
                for size in [1.0, 1.2] {
                    let pose = Pose { look: [a.cos(), a.sin()], eye_size: size, eye_open: 1.2, ..Pose::REST };
                    assert!(eyes_inside(shape, &pose), "{shape:?} look {a} size {size}");
                }
            }
            assert!(eyes_inside(shape, &Pose::REST), "{shape:?} at rest");
        }
    }

    #[test]
    fn faces_fit_with_readable_eyes() {
        assert!(face(Shape::Blob).spread > 0.85, "the brand blob keeps a full face");
        for shape in Shape::ALL {
            let fitted = face(shape);
            assert!(fitted.spread >= 0.45, "{shape:?} eyes too small: {}", fitted.spread);
            assert_eq!(face(shape), fitted, "cached");
        }
    }

    #[test]
    fn eyes_do_not_touch_each_other() {
        for shape in Shape::ALL {
            let pose = Pose { eye_size: 1.2, eye_open: 1.2, ..Pose::REST };
            let figure = Figure::build(Body::of(shape), &pose);
            let [near, far] = &figure.eyes;
            assert!(near.iter().all(|p| !contains(far, *p)), "{shape:?}");
            assert!(far.iter().all(|p| !contains(near, *p)), "{shape:?}");
        }
    }

    #[test]
    fn look_is_clamped_to_the_unit_disk() {
        assert_eq!(clamp_look([0.3, -0.4]), [0.3, -0.4]);
        let far = clamp_look([5.0, 5.0]);
        assert!(((far[0] * far[0] + far[1] * far[1]).sqrt() - 1.0).abs() < 1e-5);
        assert_eq!(clamp_look([f32::NAN, f32::INFINITY]), [0.0, 0.0]);
        // an out-of-range look draws like the clamped one
        let wild = Figure::build(Body::of(Shape::Blob), &Pose { look: [9.0, 0.0], ..Pose::REST });
        let edge = Figure::build(Body::of(Shape::Blob), &Pose { look: [1.0, 0.0], ..Pose::REST });
        assert_eq!(wild.eyes, edge.eyes);
    }

    #[test]
    fn closed_eyes_are_slits() {
        let shut = Figure::build(Body::of(Shape::Blob), &Pose { eye_open: 0.0, ..Pose::REST });
        let open = Figure::build(Body::of(Shape::Blob), &Pose::REST);
        let area = |e: &EyePolygon| outline::signed_area(e).abs();
        assert!(area(&shut.eyes[0]) < area(&open.eyes[0]) * 0.1);
        assert!(area(&shut.eyes[0]) > 0.0);
    }

    #[test]
    fn morph_ends_match_their_shapes() {
        let pose = Pose::REST;
        let start = Figure::build(Body { from: Shape::Star, to: Shape::Heart, blend: 0.0 }, &pose);
        let end = Figure::build(Body { from: Shape::Star, to: Shape::Heart, blend: 1.0 }, &pose);
        assert_eq!(start.body, Figure::build(Body::of(Shape::Star), &pose).body);
        assert_eq!(end.body, Figure::build(Body::of(Shape::Heart), &pose).body);
    }

    #[test]
    fn squash_keeps_the_base() {
        let flat = Figure::build(Body::of(Shape::Blob), &Pose { stretch: 0.7, ..Pose::REST });
        let base = |f: &Figure| f.body.iter().fold(f32::MIN, |m, p| m.max(p[1]));
        let top = |f: &Figure| f.body.iter().fold(f32::MAX, |m, p| m.min(p[1]));
        let rest = Figure::build(Body::of(Shape::Blob), &Pose::REST);
        assert!((base(&flat) - base(&rest)).abs() < 0.02);
        assert!(top(&flat) > top(&rest) + 0.2);
    }

    #[test]
    fn paints_one_even_odd_path() {
        let figure = Figure::build(Body::of(Shape::Cloud), &Pose::REST);
        let drawing = figure.paint(Painter::new(), 30.0, 30.0, 18.0, Hsla::default()).finish();
        // begin, 3 loops of move + curves + close, fill style, fill
        assert_eq!(drawing.ops.len(), 1 + (OUTLINE_POINTS + 2) + 2 * (EYE_POINTS + 2) + 2);
    }
}
