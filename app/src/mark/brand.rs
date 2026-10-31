//! The brand mascot files in `assets/brand/`: the default bot (a white
//! `blob`) at rest, looking straight ahead, from the same baked frame
//! `paint_rest` fills. The bake is a byte-quantised polygon, which shows
//! its facets at icon sizes, so each ring is drawn as quadratic curves
//! through its edges' midpoints (its points are the control points). SVG
//! from those curves; PNG rasterised from the same curves at 256, 512 and
//! 1024. Light is the ink a light theme draws (black), dark
//! a dark theme's (white); both on a transparent ground.
//!
//! Regenerate: `cargo test -p rox brand::export -- --ignored` (writes to
//! `BLOB_EXPORT_DIR`, default `assets/brand/`). `brand::svgs_are_current`
//! fails when the committed SVGs no longer match the bake.

use std::path::PathBuf;

use super::paint::{rest_bytes, rest_rings, GazeState};
use super::ink_of;

const SHAPE: &str = "blob";
const COLOR: &str = "#FFFFFF";
const PNG_SIZES: [u32; 3] = [256, 512, 1024];

/// The resting outline in the bake's 0–255 square, then the square it's
/// framed in: the outline's bounds, squared and centred, plus a margin.
fn outline() -> ([Vec<[f64; 2]>; 3], [f64; 3]) {
    let bytes = rest_bytes(SHAPE).expect("the blob's rest frames are baked");
    let rings = rest_rings(&bytes, &GazeState::default(), 255.0);
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for [x, y] in rings.iter().flatten() {
        (x0, y0, x1, y1) = (x0.min(*x), y0.min(*y), x1.max(*x), y1.max(*y));
    }
    let side = (x1 - x0).max(y1 - y0) * 1.1;
    let frame = [(x0 + x1 - side) / 2.0, (y0 + y1 - side) / 2.0, side];
    (rings, frame)
}

fn variants() -> [(&'static str, String); 2] {
    [("light", ink_of(COLOR, true)), ("dark", ink_of(COLOR, false))]
}

/// Each ring as `(start, [(control, end)])`, framed: a closed chain of
/// quadratics from midpoint to midpoint.
fn curves() -> (Vec<([f64; 2], Vec<([f64; 2], [f64; 2])>)>, f64) {
    let (rings, [fx, fy, side]) = outline();
    let mid = |a: [f64; 2], b: [f64; 2]| [(a[0] + b[0]) / 2.0 - fx, (a[1] + b[1]) / 2.0 - fy];
    let rings = rings
        .iter()
        .map(|ring| {
            let n = ring.len();
            let segs = (0..n)
                .map(|i| {
                    let [cx, cy] = ring[(i + 1) % n];
                    ([cx - fx, cy - fy], mid(ring[(i + 1) % n], ring[(i + 2) % n]))
                })
                .collect();
            (mid(ring[0], ring[1 % n]), segs)
        })
        .collect();
    (rings, side)
}

fn svg(ink: &str) -> String {
    let (rings, side) = curves();
    let mut d = String::new();
    for (start, segs) in &rings {
        d.push_str(&format!("M{:.2} {:.2}", start[0], start[1]));
        for (c, e) in segs {
            d.push_str(&format!("Q{:.2} {:.2} {:.2} {:.2}", c[0], c[1], e[0], e[1]));
        }
        d.push('Z');
    }
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {side:.2} {side:.2}\" width=\"1024\" height=\"1024\">\
         <path fill=\"{ink}\" fill-rule=\"evenodd\" d=\"{d}\"/></svg>\n"
    )
}

fn png(ink: &str, size: u32) -> Vec<u8> {
    let (rings, side) = curves();
    let mut pb = tiny_skia::PathBuilder::new();
    for (start, segs) in &rings {
        pb.move_to(start[0] as f32, start[1] as f32);
        for (c, e) in segs {
            pb.quad_to(c[0] as f32, c[1] as f32, e[0] as f32, e[1] as f32);
        }
        pb.close();
    }
    let path = pb.finish().expect("a closed outline");
    let hex = u32::from_str_radix(ink.trim_start_matches('#'), 16).expect("a #rrggbb ink");
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8, 255);
    paint.anti_alias = true;
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("a nonzero size");
    let k = size as f32 / side as f32;
    pixmap.fill_path(&path, &paint, tiny_skia::FillRule::EvenOdd, tiny_skia::Transform::from_scale(k, k), None);
    pixmap.encode_png().expect("png encoding")
}

fn out_dir() -> PathBuf {
    std::env::var_os("BLOB_EXPORT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../assets/brand"))
}

#[test]
#[ignore = "writes assets/brand/; run it to regenerate them"]
fn export() {
    let dir = out_dir();
    std::fs::create_dir_all(&dir).unwrap();
    for (name, ink) in variants() {
        std::fs::write(dir.join(format!("mascot-{name}.svg")), svg(&ink)).unwrap();
        for size in PNG_SIZES {
            std::fs::write(dir.join(format!("mascot-{name}-{size}.png")), png(&ink, size)).unwrap();
        }
    }
}

#[test]
fn svgs_are_current() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../assets/brand");
    for (name, ink) in variants() {
        let path = dir.join(format!("mascot-{name}.svg"));
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(committed == svg(&ink), "{} is stale: regenerate it (brand::export)", path.display());
    }
}
