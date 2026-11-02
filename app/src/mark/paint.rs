//! `mark/markpaint.ts` → `Painter` ops, plus `blob.tsx`'s resting-frame
//! painter (`paintRest`/`restOf`/`gazeSet`/`gazeAdvance`) and `statusfx.tsx`'s
//! `drawFx` ambient overlay.

use std::cell::RefCell;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::str::FromStr;

use nsh_core::canvas::{svg_path, CanvasTextAlign, CanvasTextBaseline, FillRule, Path};
use nsh_core::prelude::*;
use nsh_core::Hsla;

use super::rest_data::{REST, REST_EN, REST_LAYERS, REST_N};
use super::MarkTree;

const TAU: f64 = PI * 2.0;

const REST_FB: usize = (REST_N + 2 * REST_EN) * 2;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

thread_local! {
    static PATHS: RefCell<HashMap<String, Path>> = RefCell::new(HashMap::new());
    static RESTS: RefCell<HashMap<&'static str, std::rc::Rc<[u8]>>> = RefCell::new(HashMap::new());
}

/// `pathOf` — cached `svg_path`.
fn path_of(data: &str) -> Path {
    PATHS.with(|paths| {
        let mut paths = paths.borrow_mut();
        if let Some(p) = paths.get(data) {
            return p.clone();
        }
        if paths.len() >= 256
            && let Some(k) = paths.keys().next().cloned() {
                paths.remove(&k);
        }
        let p = svg_path(data);
        paths.insert(data.to_owned(), p.clone());
        p
    })
}

/// `attrs[key]` as a number.
fn number_attr(tree: &MarkTree, key: &str, fallback: f64) -> f64 {
    tree.attr(key)
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(fallback)
}

/// A CSS color string → `Hsla` (black when unparseable).
fn hsla_of(s: &str) -> Hsla {
    Hsla::from_str(s).unwrap_or(Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.0,
        a: 1.0,
    })
}

/// `applyTransform` — `matrix`/`translate`/`scale`/`rotate(cx cy)`.
fn apply_transform(mut p: Painter, value: &str) -> Painter {
    let mut rest = value;
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim();
        let name_start = name
            .rfind(|c: char| !c.is_ascii_alphabetic())
            .map(|i| i + 1)
            .unwrap_or(0);
        let name = &name[name_start..];
        let Some(close) = rest[open..].find(')') else { break };
        let raw = &rest[open + 1..open + close];
        let args: Vec<f64> = raw
            .split([' ', ','])
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse::<f64>().ok())
            .collect();
        let f = |i: usize| -> f32 { args.get(i).copied().unwrap_or(0.0) as f32 };
        match name {
            "matrix" if args.len() == 6 => {
                p = p.transform([f(0), f(1), f(2), f(3), f(4), f(5)]);
            }
            "translate" => {
                p = p.translate(f(0), f(1));
            }
            "scale" => {
                let y = if args.len() > 1 { f(1) } else { f(0) };
                p = p.scale(f(0), y);
            }
            "rotate" => {
                let angle = (args.first().copied().unwrap_or(0.0) * PI / 180.0) as f32;
                if args.len() >= 3 {
                    p = p.translate(f(1), f(2)).rotate(angle).translate(-f(1), -f(2));
                } else {
                    p = p.rotate(angle);
                }
            }
            _ => {}
        }
        rest = &rest[open + close + 1..];
    }
    p
}

/// `collectDefs` — id → node.
fn collect_defs<'a>(tree: &'a MarkTree, defs: &mut HashMap<&'a str, &'a MarkTree>) {
    if let Some(id) = tree.attr("id") {
        defs.insert(id, tree);
    }
    for child in &tree.children {
        collect_defs(child, defs);
    }
}

/// `maskPath` — the mask's white rect minus its black eye holes, as a Path.
fn mask_path(mask: &MarkTree) -> Path {
    let rect = mask
        .children
        .iter()
        .find(|c| c.tag == "rect")
        .unwrap_or(mask);
    let x = number_attr(rect, "x", 0.0);
    let y = number_attr(rect, "y", 0.0);
    let w = number_attr(rect, "width", 0.0);
    let h = number_attr(rect, "height", 0.0);
    let holes: String = mask
        .children
        .iter()
        .filter(|c| c.tag == "path" && c.attr("fill") == Some("#000"))
        .map(|c| c.attr("d").unwrap_or("").to_owned())
        .collect();
    path_of(&format!("M{} {}H{}V{}H{}Z{}", x, y, x + w, y + h, x, holes))
}

// ---------------------------------------------------------------------------
// paintTree
// ---------------------------------------------------------------------------

fn paint_node<'a>(
    mut p: Painter,
    tree: &'a MarkTree,
    defs: &HashMap<&'a str, &'a MarkTree>,
    inherited_fill: &str,
    parent_alpha: f64,
) -> Painter {
    if tree.tag == "defs" || tree.tag == "mask" {
        return p;
    }
    let opacity = tree
        .attr("opacity")
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(1.0);
    let alpha = parent_alpha * if opacity.is_finite() { opacity } else { 1.0 };
    p = p.save();
    if let Some(t) = tree.attr("transform") {
        p = apply_transform(p, t);
    }
    let mask_id = tree.attr("mask").and_then(|m| {
        let h = m.find('#')?;
        let end = m[h + 1..]
            .find(|c: char| c == ')' || c.is_whitespace())
            .map(|i| h + 1 + i)
            .unwrap_or(m.len());
        Some(m[h + 1..end].to_owned())
    });
    let mask = mask_id.as_deref().and_then(|id| defs.get(id));
    if let Some(mask) = mask {
        p = p.save().clip_path(&mask_path(mask), FillRule::EvenOdd);
    }

    match tree.tag {
        "svg" | "g" => {
            for child in &tree.children {
                p = paint_node(p, child, defs, inherited_fill, alpha);
            }
        }
        "use" => {
            let href = tree
                .attr("href")
                .or_else(|| tree.attr("xlink:href"))
                .unwrap_or("")
                .trim_start_matches('#');
            if let Some(reff) = defs.get(href) {
                let fill = tree.attr("fill").unwrap_or(inherited_fill);
                p = paint_node(p, reff, defs, fill, alpha);
            }
        }
        "path" => {
            let d = tree.attr("d").unwrap_or("");
            if !d.is_empty() {
                let path = path_of(d);
                let fill = tree.attr("fill").unwrap_or(inherited_fill);
                p = p.global_alpha(alpha as f32);
                if fill != "none" {
                    p = p.fill_color(hsla_of(fill)).fill_path(
                        &path,
                        if tree.attr("fill-rule") == Some("evenodd") {
                            FillRule::EvenOdd
                        } else {
                            FillRule::NonZero
                        },
                    );
                }
                if let Some(stroke) = tree.attr("stroke")
                    && stroke != "none" {
                        p = p
                            .stroke_color(hsla_of(stroke))
                            .line_width(number_attr(tree, "stroke-width", 1.0) as f32)
                            .line_cap(match tree.attr("stroke-linecap") {
                                Some("round") => nsh_core::canvas::LineCap::Round,
                                Some("square") => nsh_core::canvas::LineCap::Square,
                                _ => nsh_core::canvas::LineCap::Butt,
                            })
                            .line_join(match tree.attr("stroke-linejoin") {
                                Some("round") => nsh_core::canvas::LineJoin::Round,
                                Some("bevel") => nsh_core::canvas::LineJoin::Bevel,
                                _ => nsh_core::canvas::LineJoin::Miter,
                            })
                            .stroke_path(&path);
                }
            }
        }
        "circle" => {
            let cx = number_attr(tree, "cx", 0.0) as f32;
            let cy = number_attr(tree, "cy", 0.0) as f32;
            let r = number_attr(tree, "r", 0.0) as f32;
            let fill = tree.attr("fill").unwrap_or(inherited_fill);
            p = p.global_alpha(alpha as f32).begin_path().arc(cx, cy, r, 0.0, TAU as f32);
            if fill != "none" {
                p = p.fill_color(hsla_of(fill)).fill();
            }
            if let Some(stroke) = tree.attr("stroke")
                && stroke != "none" {
                    let dash: Vec<Points> = tree
                        .attr("stroke-dasharray")
                        .unwrap_or("")
                        .split([' ', ','])
                        .filter(|s| !s.is_empty())
                        .filter_map(|s| s.parse::<f32>().ok())
                        .map(Points)
                        .collect();
                    p = p
                        .stroke_color(hsla_of(stroke))
                        .line_width(number_attr(tree, "stroke-width", 1.0) as f32)
                        .line_dash(&dash)
                        .stroke()
                        .line_dash(&[]);
            }
        }
        _ => {}
    }
    if mask.is_some() {
        p = p.restore();
    }
    p.restore()
}

/// `paintTree(ctx, tree, x, y, px)`.
pub(crate) fn paint_tree(p: Painter, tree: &MarkTree, x: f64, y: f64, px: f64) -> Painter {
    if tree.tag != "svg" {
        return p;
    }
    let vb: Vec<f64> = tree
        .attr("viewBox")
        .unwrap_or("0 0 1 1")
        .split([' ', ','])
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<f64>().ok())
        .collect();
    let vx = vb.first().copied().unwrap_or(0.0);
    let vy = vb.get(1).copied().unwrap_or(0.0);
    let w = vb.get(2).copied().unwrap_or(1.0);
    let h = vb.get(3).copied().unwrap_or(w);
    let scale = px / w.max(h);
    let mut defs = HashMap::new();
    collect_defs(tree, &mut defs);
    let mut p = p
        .save()
        .translate((x - vx * scale) as f32, (y - vy * scale) as f32)
        .scale(scale as f32, scale as f32);
    for child in &tree.children {
        p = paint_node(p, child, &defs, "#000", 1.0);
    }
    p.restore()
}

// ---------------------------------------------------------------------------
// `restOf`/`paintRest`/`GazeState` (blob.tsx)
// ---------------------------------------------------------------------------

/// `atob` for the `REST` strings.
fn b64(data: &str) -> Vec<u8> {
    fn val(c: u8) -> u32 {
        match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            _ => 0,
        }
    }
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    let mut acc = 0u32;
    let mut nbits = 0u32;
    for &c in data.as_bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        acc = (acc << 6) | val(c);
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((acc >> nbits) as u8);
            acc &= (1 << nbits) - 1;
        }
    }
    out
}

/// `restOf(shape)` — decoded resting frames (`None` when unbaked),
/// decoded once per shape: a resting mark repaints on every gaze step.
pub(crate) fn rest_bytes(shape: &str) -> Option<std::rc::Rc<[u8]>> {
    let (name, data) = REST.iter().find(|(n, _)| *n == shape)?;
    Some(RESTS.with(|c| c.borrow_mut().entry(name).or_insert_with(|| b64(data).into()).clone()))
}

/// `GazeState` — `se()`'s gaze state mirrored for `paintRest`.
#[derive(Clone, Default)]
pub(crate) struct GazeState {
    pub held: bool,
    pub x: f64,
    pub y: f64,
    pub cx: f64,
    pub cy: f64,
    pub hold: f64,
}

fn clamp(v: f64) -> f64 {
    v.clamp(-1.0, 1.0)
}

/// `gazeSet`.
pub(crate) fn gaze_set(s: &mut GazeState, gaze: Option<V2g>) {
    match gaze {
        None => s.held = false,
        Some([gx, gy]) => {
            s.x = clamp(gx);
            s.y = clamp(gy);
            if s.hold == 0.0 {
                s.cx = s.x;
                s.cy = s.y;
            }
            s.held = true;
        }
    }
}

pub(crate) type V2g = [f64; 2];

fn near(from: f64, to: f64, k: f64) -> f64 {
    let v = from + (to - from) * k;
    if (to - v).abs() < 0.001 {
        to
    } else {
        v
    }
}

/// `gazeAdvance(s, dt)`.
pub(crate) fn gaze_advance(s: &mut GazeState, dt: f64) {
    if !s.held && s.hold == 0.0 {
        return;
    }
    let k = 1.0 - 0.86f64.powf(dt.min(0.1) * 60.0);
    s.cx = near(s.cx, s.x, k);
    s.cy = near(s.cy, s.y, k);
    s.hold = near(s.hold, if s.held { 1.0 } else { 0.0 }, k);
}

/// `paintRest(ctx, bytes, s, x, y, px, ink)` — one even-odd fill of the
/// baked pose (`rest_rings`).
pub(crate) fn paint_rest(
    p: Painter,
    bytes: &[u8],
    s: &GazeState,
    x: f64,
    y: f64,
    px: f64,
    ink: &str,
) -> Painter {
    let mut p = p.begin_path();
    for ring in rest_rings(bytes, s, px) {
        for (i, [ax, ay]) in ring.into_iter().enumerate() {
            if i == 0 {
                p = p.move_to((x + ax) as f32, (y + ay) as f32);
            } else {
                p = p.line_to((x + ax) as f32, (y + ay) as f32);
            }
        }
        p = p.close_path();
    }
    p.global_alpha(1.0)
        .fill_color(hsla_of(ink))
        .fill_with(FillRule::EvenOdd)
}

/// The baked pose at gaze `s`, in a `px` square: its closed rings (the
/// body, then each eye; even-odd cuts the eyes out), bilinear in each hold
/// layer's gaze grid, linear between layers (layer 0 is the rest frame).
/// `paint_rest` fills them; the brand exporter writes them as SVG.
pub(crate) fn rest_rings(bytes: &[u8], s: &GazeState, px: f64) -> [Vec<[f64; 2]>; 3] {
    let layer = |index: i64| -> ([f64; 4], [f64; 4]) {
        if index < 0 {
            return ([0.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]);
        }
        let mut base = 1usize;
        for layer in &REST_LAYERS[..index as usize] {
            base += layer.1 * layer.1;
        }
        let n = REST_LAYERS[index as usize].1 as f64;
        let fx = (s.cx + 1.0) / 2.0 * (n - 1.0);
        let fy = (s.cy + 1.0) / 2.0 * (n - 1.0);
        let x0 = (n - 2.0).min(fx.floor()).max(0.0);
        let y0 = (n - 2.0).min(fy.floor()).max(0.0);
        let (ax, ay) = (fx - x0, fy - y0);
        let f = base as f64 + y0 * n + x0;
        (
            [f, f + 1.0, f + n, f + n + 1.0],
            [(1.0 - ax) * (1.0 - ay), ax * (1.0 - ay), (1.0 - ax) * ay, ax * ay],
        )
    };
    let mut index = 0usize;
    while index < REST_LAYERS.len() - 1 && REST_LAYERS[index].0 < s.hold {
        index += 1;
    }
    let h0 = if index == 0 { 0.0 } else { REST_LAYERS[index - 1].0 };
    let t = ((s.hold - h0) / (REST_LAYERS[index].0 - h0)).clamp(0.0, 1.0);
    let (fa, wa) = layer(index as i64 - 1);
    let (fb, wb) = layer(index as i64);
    let k = px / 255.0;
    let at = |j: usize| -> f64 {
        let mut a = 0.0;
        let mut b = 0.0;
        for i in 0..4 {
            let fi = fa[i] as usize * REST_FB + j;
            if fi < bytes.len() {
                a += bytes[fi] as f64 * wa[i];
            }
        }
        for i in 0..4 {
            let fi = fb[i] as usize * REST_FB + j;
            if fi < bytes.len() {
                b += bytes[fi] as f64 * wb[i];
            }
        }
        (a + (b - a) * t) * k
    };
    [(0usize, REST_N), (REST_N, REST_EN), (REST_N + REST_EN, REST_EN)].map(|(from, count)| {
        (0..count)
            .map(|i| {
                let j = (from + i) * 2;
                [at(j), at(j + 1)]
            })
            .collect()
    })
}

// ---------------------------------------------------------------------------
// `drawFx` — statusfx.tsx's ambient overlay (AgentFx around the mark)
// ---------------------------------------------------------------------------

/// `drawFx(fx, ctx, w, h, t, ink)`.
pub(crate) fn draw_fx(mut p: Painter, fx: &str, w: f64, h: f64, t: f64, ink: &str) -> Painter {
    let (cx, cy) = (w / 2.0, h / 2.0);
    let r = w * 0.30;
    let ink_c = hsla_of(ink);
    p = p.fill_color(ink_c).stroke_color(ink_c);
    match fx {
        "dots" => {
            for i in 0..3 {
                let pp = (t * 1.7 + i as f64 * 0.33) % 1.0;
                p = p
                    .global_alpha((0.35 + 0.65 * (pp * PI).sin()) as f32)
                    .begin_path()
                    .arc(
                        (cx + (i as f64 - 1.0) * r * 0.5) as f32,
                        (cy + r * 0.45 - (pp * PI).sin() * r * 0.18) as f32,
                        (r * 0.09) as f32,
                        0.0,
                        TAU as f32,
                    )
                    .fill();
            }
        }
        "orbit" => {
            let a = t * TAU / 1.7;
            for k in 0..4 {
                let aa = a - k as f64 * 0.42;
                p = p
                    .global_alpha(((1.0 - k as f64 / 4.0) * 0.9) as f32)
                    .begin_path()
                    .arc(
                        (cx + aa.cos() * r * 1.15) as f32,
                        (cy + aa.sin() * r * 1.15) as f32,
                        (r * 0.085 * (1.0 - k as f64 * 0.15)) as f32,
                        0.0,
                        TAU as f32,
                    )
                    .fill();
            }
        }
        "radar" => {
            for i in 0..3 {
                let pp = (t * 0.55 + i as f64 / 3.0) % 1.0;
                p = p
                    .global_alpha(((1.0 - pp) * 0.55) as f32)
                    .line_width(1.0)
                    .begin_path()
                    .arc(
                        cx as f32,
                        cy as f32,
                        (r * (0.55 + pp * 0.95)) as f32,
                        0.0,
                        TAU as f32,
                    )
                    .stroke();
            }
        }
        "progress" => {
            let a = t * TAU / 1.15;
            p = p
                .global_alpha(0.25)
                .line_width((r * 0.09) as f32)
                .begin_path()
                .arc(cx as f32, cy as f32, (r * 1.05) as f32, 0.0, TAU as f32)
                .stroke()
                .global_alpha(0.95)
                .begin_path()
                .arc(
                    cx as f32,
                    cy as f32,
                    (r * 1.05) as f32,
                    a as f32,
                    (a + TAU * 0.22) as f32,
                )
                .stroke();
        }
        "gather" => {
            for i in 0..5 {
                let ang = i as f64 * (TAU / 5.0) + 0.5;
                let pp = (t * 0.9 + i as f64 * 0.17) % 1.0;
                let rad = r * (1.6 - 1.1 * pp);
                p = p
                    .global_alpha((0.3 + 0.6 * pp) as f32)
                    .begin_path()
                    .arc(
                        (cx + ang.cos() * rad) as f32,
                        (cy + ang.sin() * rad) as f32,
                        (r * 0.075) as f32,
                        0.0,
                        TAU as f32,
                    )
                    .fill();
            }
        }
        "wave" => {
            for i in 0..5 {
                let hh = r * (0.18 + 0.5 * (0.5 + 0.5 * (t * 7.0 + i as f64 * 1.15).sin()));
                p = p
                    .global_alpha(0.9)
                    .begin_path()
                    .round_rect(
                        bounds(
                            point(
                                px((cx + (i as f64 - 2.0) * r * 0.32 - r * 0.05) as f32),
                                px((cy + r * 0.35 - hh / 2.0) as f32),
                            ),
                            size(px((r * 0.1) as f32), px(hh as f32)),
                        ),
                        px((r * 0.05) as f32),
                    )
                    .fill();
            }
        }
        "send" => {
            for k in 0..3 {
                let pp = (t * 1.05 - k as f64 * 0.09) % 1.0;
                if pp < 0.0 {
                    continue;
                }
                p = p
                    .global_alpha(((1.0 - k as f64 / 3.0) * (1.0 - pp * 0.7)) as f32)
                    .begin_path()
                    .arc(
                        (cx + pp * pp * r * 1.5) as f32,
                        (cy - pp * r * 1.35) as f32,
                        (r * 0.085) as f32,
                        0.0,
                        TAU as f32,
                    )
                    .fill();
            }
        }
        "receive" => {
            let pp = (t * 1.1) % 1.0;
            let ease = pp * pp;
            p = p
                .global_alpha(0.9)
                .begin_path()
                .arc(
                    cx as f32,
                    (cy - r * 1.5 + ease * r * 1.5) as f32,
                    (r * 0.085) as f32,
                    0.0,
                    TAU as f32,
                )
                .fill()
                .global_alpha(0.35)
                .begin_path()
                .arc(
                    cx as f32,
                    (cy - r * 1.5 + ease * ease * r * 1.5 - r * 0.14) as f32,
                    (r * 0.06) as f32,
                    0.0,
                    TAU as f32,
                )
                .fill();
        }
        "dock" => {
            let pp = (t * 0.9) % 1.0;
            let yy = if pp < 0.5 { pp * 2.0 } else { (1.0 - pp) * 2.0 };
            p = p
                .global_alpha(0.9)
                .begin_path()
                .arc(
                    cx as f32,
                    (cy - yy * r * 1.3) as f32,
                    (r * 0.085) as f32,
                    0.0,
                    TAU as f32,
                )
                .fill();
        }
        "ball" => {
            let yy = (t * 5.5).sin().abs();
            p = p
                .global_alpha(0.9)
                .begin_path()
                .arc(
                    cx as f32,
                    (cy + r * 1.1 - yy * r * 0.75) as f32,
                    (r * 0.1) as f32,
                    0.0,
                    TAU as f32,
                )
                .fill();
        }
        "pencil" => {
            let pp = (t * 1.3) % 1.0;
            let n = 14;
            p = p.global_alpha(0.9).line_width((r * 0.07) as f32).begin_path();
            let limit = (n as f64 * pp).floor() as i64;
            for i in 0..=limit {
                let u = i as f64 / n as f64;
                let xx = cx - r * 0.55 + u * r * 1.1;
                let yy = cy + r * 0.5 + (u * 9.0).sin() * r * 0.09;
                p = if i == 0 {
                    p.move_to(xx as f32, yy as f32)
                } else {
                    p.line_to(xx as f32, yy as f32)
                };
            }
            p = p.stroke();
        }
        "bang" => {
            let s = 1.0 + (t * 6.0).sin() * 0.12;
            p = p
                .global_alpha(0.95)
                .font(&format!("700 {}px -apple-system, system-ui", r * 0.85 * s))
                .text_align(CanvasTextAlign::Center)
                .text_baseline(CanvasTextBaseline::Middle)
                .fill_text("!", cx as f32, (cy - r * 0.55) as f32);
        }
        "standby" => {
            let pp = 0.35 + 0.55 * (0.5 + 0.5 * (t * 2.4).sin());
            p = p
                .global_alpha(pp as f32)
                .line_width((r * 0.09) as f32)
                .begin_path()
                .arc(
                    cx as f32,
                    (cy - r * 0.35) as f32,
                    (r * 0.3) as f32,
                    (PI * 0.35) as f32,
                    (PI * 2.65) as f32,
                )
                .stroke()
                .begin_path()
                .move_to(cx as f32, (cy - r * 0.72) as f32)
                .line_to(cx as f32, (cy - r * 0.38) as f32)
                .stroke();
        }
        _ => {}
    }
    p.global_alpha(1.0)
}
