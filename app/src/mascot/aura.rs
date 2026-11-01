//! The ambient effects drawn around an avatar while some states last
//! (`Aura`): a few strokes and dots per frame, sized from the canvas.

use std::f32::consts::{PI, TAU};

use nsh_core::canvas::LineCap;
use nsh_core::prelude::*;

use super::states::Aura;

/// The body radius on an aura canvas: the canvas is 1.5 cells wide and the
/// body 0.82 of a cell across.
fn body_radius(width: f32, height: f32) -> f32 {
    width.min(height) / 1.5 * 0.41
}

/// Draws `aura` at `seconds` on a `width`×`height` canvas centered on the
/// avatar, in `ink`.
pub(crate) fn draw_aura(mut p: Painter, aura: Aura, width: f32, height: f32, seconds: f64, ink: Hsla) -> Painter {
    let (cx, cy) = (width / 2.0, height / 2.0);
    let radius = body_radius(width, height);
    // wrap long clocks so f32 keeps its precision
    let t = (seconds % 3600.0) as f32;
    let dot = |p: Painter, x: f32, y: f32, r: f32, alpha: f32| {
        p.save()
            .global_alpha(alpha.clamp(0.0, 1.0))
            .begin_path()
            .arc(x, y, r.max(0.1), 0.0, TAU)
            .fill_color(ink)
            .fill()
            .restore()
    };
    let ring = |p: Painter, r: f32, from: f32, to: f32, line: f32, alpha: f32| {
        p.save()
            .global_alpha(alpha.clamp(0.0, 1.0))
            .begin_path()
            .arc(cx, cy, r, from, to)
            .stroke_color(ink)
            .line_width(line)
            .line_cap(LineCap::Round)
            .stroke()
            .restore()
    };
    match aura {
        Aura::Orbit => {
            for k in 0..2 {
                let a = t * 2.2 + PI * k as f32;
                let depth = 0.5 + 0.5 * a.sin();
                let (x, y) = (cx + radius * 1.3 * a.cos(), cy + radius * 0.5 * a.sin() - radius * 0.1);
                p = dot(p, x, y, radius * (0.07 + 0.04 * depth), 0.35 + 0.65 * depth);
            }
        }
        Aura::Pulse => {
            for k in 0..2 {
                let phase = (t / 1.6 + 0.5 * k as f32).fract();
                p = ring(p, radius * (1.05 + 0.35 * phase), 0.0, TAU, radius * 0.06, 0.8 * (1.0 - phase));
            }
        }
        Aura::Sweep => {
            let head = t * 3.0;
            for k in 0..3 {
                let tail = head - 0.3 * (k + 1) as f32;
                p = ring(p, radius * 1.25, tail, tail + 0.3, radius * 0.08, 0.9 - 0.3 * k as f32);
            }
        }
        Aura::Progress => {
            let start = t * 4.0;
            let span = 0.6 + 2.2 * (0.5 + 0.5 * (t * 2.0).sin());
            p = ring(p, radius * 1.22, start, start + span, radius * 0.08, 0.9);
        }
        Aura::Twinkle => {
            const SPOTS: [(f32, f32); 4] = [(-0.9, 0.0), (0.4, 0.35), (2.2, 0.6), (3.6, 0.15)];
            for (angle, delay) in SPOTS {
                let phase = (t / 1.4 + delay).fract();
                let size = radius * 0.16 * (PI * phase).sin();
                if size <= 0.05 {
                    continue;
                }
                let (x, y) = (cx + radius * 1.32 * angle.cos(), cy + radius * 1.32 * angle.sin());
                p = p.begin_path();
                for i in 0..8 {
                    let a = PI / 4.0 * i as f32;
                    let r = if i % 2 == 0 { size } else { size * 0.32 };
                    let (px_, py_) = (x + r * a.cos(), y + r * a.sin());
                    p = if i == 0 { p.move_to(px_, py_) } else { p.line_to(px_, py_) };
                }
                p = p.close_path().fill_color(ink).fill();
            }
        }
        Aura::Thought => {
            for i in 0..3 {
                let f = i as f32;
                let lit = 0.5 + 0.5 * (TAU * (t / 1.5 - f / 3.0)).sin();
                let (x, y) = (cx + radius * (0.95 + 0.3 * f), cy - radius * (0.95 + 0.28 * f));
                p = dot(p, x, y, radius * (0.06 + 0.035 * f), 0.3 + 0.7 * lit);
            }
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Aura; 6] = [Aura::Orbit, Aura::Pulse, Aura::Sweep, Aura::Progress, Aura::Twinkle, Aura::Thought];

    #[test]
    fn every_aura_draws_something_over_time() {
        for aura in ALL {
            for frame in 0..60 {
                let drawing = draw_aura(Painter::new(), aura, 66.0, 66.0, frame as f64 / 30.0, Hsla::default()).finish();
                if aura != Aura::Twinkle {
                    assert!(!drawing.ops.is_empty(), "{aura:?}");
                }
            }
        }
    }

    #[test]
    fn long_clocks_are_fine() {
        for aura in ALL {
            let _ = draw_aura(Painter::new(), aura, 54.0, 54.0, 1.0e9, Hsla::default());
        }
    }
}
