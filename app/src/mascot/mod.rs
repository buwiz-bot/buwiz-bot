//! The bot mascot: the animated avatar every bot wears. Written from
//! scratch for Buwiz Bot, following the brand art in `assets/brand/` (a
//! round ink body with two slanted pill eyes cut out of it).
//!
//! - `outline`: the thirteen body shapes, generated at run time.
//! - `colors`: the swatches and the ink a theme draws them in.
//! - `states`: the activity states, their motions and ambient effects.
//! - `figure`: one frame (a `Pose` on a body) and its painting.
//! - `animator`: per-avatar timing: states, blinks, gaze, gestures, morphs.
//! - `aura`: the ambient effects around an avatar.
//!
//! `avatar.rs` hosts it in the UI and `statusfx.rs` draws the auras.

mod animator;
mod aura;
mod colors;
mod figure;
mod outline;
mod states;

pub(crate) use animator::{seed_from, Animator};
pub(crate) use aura::draw_aura;
pub(crate) use colors::{darken, ink_of, norm_color, BOT_COLORS};
pub(crate) use outline::{Shape, BOT_SHAPES};
pub(crate) use states::{aura_of, is_state, state, Gesture, StateSpec, STATES};

/// A look direction: x right, y up, each in `[-1, 1]`.
pub(crate) type Look = [f64; 2];

#[cfg(test)]
mod tests {
    use super::animator::still_pose;
    use super::figure::{Body, Figure};
    use super::*;
    use nsh_core::prelude::*;

    #[test]
    fn every_shape_and_color_paints_in_every_state() {
        for shape in BOT_SHAPES {
            for color in BOT_COLORS {
                for light in [true, false] {
                    let ink = ink_of(color, light);
                    for spec in &STATES {
                        let mut animator = Animator::new(Shape::from_name(shape), spec, seed_from(shape), false, false);
                        animator.advance(0.4);
                        let drawing = animator.figure().paint(Painter::new(), 33.0, 33.0, 18.0, ink).finish();
                        assert!(!drawing.ops.is_empty());
                    }
                }
            }
        }
    }

    #[test]
    fn still_frame_draws_the_same_twice() {
        for shape in Shape::ALL {
            let pose = still_pose(state("idle"));
            let a = Figure::build(Body::of(shape), &pose).paint(Painter::new(), 10.0, 10.0, 8.0, ink_of("#FFFFFF", true));
            let b = Figure::build(Body::of(shape), &pose).paint(Painter::new(), 10.0, 10.0, 8.0, ink_of("#FFFFFF", true));
            assert_eq!(format!("{:?}", a.finish().ops), format!("{:?}", b.finish().ops));
        }
    }

    /// Writes contact sheets of every shape in a few states to
    /// `$MASCOT_PREVIEW_DIR` (PNG), for a look without running the app:
    /// `MASCOT_PREVIEW_DIR=/tmp/m cargo test -p rox --lib mascot::tests::preview -- --ignored`.
    #[test]
    #[ignore]
    fn preview() {
        use crate::mascot::figure::Pose;
        let Some(dir) = std::env::var_os("MASCOT_PREVIEW_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cell = 120.0f32;
        let render = |name: &str, rows: &[(&str, Pose)]| {
            let width = (cell * Shape::ALL.len() as f32) as u32;
            let height = (cell * rows.len() as f32) as u32;
            let mut pixmap = tiny_skia::Pixmap::new(width, height).unwrap();
            pixmap.fill(tiny_skia::Color::from_rgba8(24, 24, 24, 255));
            for (row, (_, pose)) in rows.iter().enumerate() {
                for (col, shape) in Shape::ALL.iter().enumerate() {
                    let figure = Figure::build(Body::of(*shape), pose);
                    let (cx, cy, r) = (cell * (col as f32 + 0.5), cell * (row as f32 + 0.5), cell * 0.3);
                    let mut path = tiny_skia::PathBuilder::new();
                    let mut add = |points: &[[f32; 2]]| {
                        let n = points.len();
                        let at = |q: [f32; 2]| (cx + q[0] * r, cy + q[1] * r);
                        let mid = |a: [f32; 2], b: [f32; 2]| at([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
                        let start = mid(points[n - 1], points[0]);
                        path.move_to(start.0, start.1);
                        for i in 0..n {
                            let (c, e) = (at(points[i]), mid(points[i], points[(i + 1) % n]));
                            path.quad_to(c.0, c.1, e.0, e.1);
                        }
                        path.close();
                    };
                    add(&figure.body);
                    for eye in &figure.eyes {
                        add(eye);
                    }
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(245, 245, 245, 255);
                    paint.anti_alias = true;
                    pixmap.fill_path(
                        &path.finish().unwrap(),
                        &paint,
                        tiny_skia::FillRule::EvenOdd,
                        tiny_skia::Transform::identity(),
                        None,
                    );
                }
            }
            pixmap.save_png(dir.join(format!("{name}.png"))).unwrap();
        };
        let names = ["idle", "thinking", "working", "reading", "waiting", "finished", "failed", "angry", "sleeping", "surprised"];
        let rows: Vec<(&str, Pose)> = names.iter().map(|n| (*n, still_pose(state(n)))).collect();
        render("states-still", &rows);
        let rows: Vec<(&str, Pose)> = names
            .iter()
            .map(|n| {
                let mut a = Animator::new(Shape::Blob, state(n), 5, false, false);
                for _ in 0..21 {
                    a.advance(1.0 / 30.0);
                }
                (*n, a.pose())
            })
            .collect();
        render("states-at-0.7s", &rows);
        let looks: Vec<(&str, Pose)> = [[-1.0, 0.0], [1.0, 0.0], [0.0, 1.0], [0.0, -1.0], [0.55, 0.5]]
            .iter()
            .map(|l| ("look", Pose { look: *l, ..Pose::REST }))
            .collect();
        render("looks", &looks);
    }
}
