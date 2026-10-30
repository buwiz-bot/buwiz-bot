//! A Pele scene as an nsh canvas drawing: shapes as path ops, labels as
//! `FillText` at the scene's own x/y (alphabetic baseline, `text-anchor` as
//! the alignment), colors read through the palette. Each label is also a
//! `CanvasNode`, and each `<a>` a pressable area for `links.rs`.

use std::f32::consts::TAU;
use std::sync::Arc;

use nsh_core::canvas::{
    CanvasFont, CanvasNode, CanvasOp, CanvasTextAlign, CanvasTextBaseline, Drawing, FillRule, LineCap,
    LineJoin, Num, Paint as CPaint, PathOp, parse_svg_path,
};
use nsh_core::prelude::*;
use nsh_core::Color;
use pele::scene::{Element, Node, Paint, declarations, unescape};
use pele::Scene;

use super::palette::PelePalette;

/// What the canvas draws for one diagram.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Drawn {
    pub drawing: Arc<Drawing>,
    pub w: f32,
    pub h: f32,
    /// Where each `click` link is, in the diagram's points, and its URL.
    pub links: Vec<(Bounds<Points>, String)>,
    /// Elements or attributes the translation does not know (tests).
    pub unknown: Vec<String>,
}

const ELEMENTS: &[&str] = &["svg", "g", "path", "rect", "text", "tspan", "circle", "polygon", "ellipse", "a", "title", "desc", "image"];
const ATTRS: &[&str] = &[
    "xmlns", "class", "viewBox", "width", "height", "style", "fill", "role", "aria-roledescription", "aria-labelledby",
    "aria-describedby", "id", "data-id", "transform", "x", "y", "rx", "ry", "d", "stroke", "stroke-width", "stroke-dasharray",
    "stroke-linecap", "stroke-linejoin", "fill-opacity", "text-anchor", "xml:space", "font-weight", "font-style",
    "font-family", "font-size", "text-decoration", "baseline-shift", "cx", "cy", "r", "points", "href", "rel", "target",
    "data-href", "pointer-events", "data-icon", "preserveAspectRatio", "opacity", "color",
];

/// The inherited presentation state (SVG's inherited properties).
#[derive(Clone)]
struct State {
    fill: String,
    stroke: String,
    stroke_width: f32,
    fill_opacity: f32,
    stroke_opacity: f32,
    // A group's `opacity`, carried down to what it holds. The canvas has no
    // layers, so overlapping children are each faded rather than faded as one.
    opacity: f32,
    underline: bool,
    dash: Option<String>,
    cap: LineCap,
    join: LineJoin,
    font_size: f32,
    weight: String,
    italic: bool,
    family: String,
    anchor: String,
    color: String,
    rx: Option<String>,
}

struct Ctx<'a> {
    palette: &'a PelePalette,
    aliases: Vec<(String, String)>,
    ops: Vec<CanvasOp>,
    nodes: Vec<CanvasNode>,
    links: Vec<(Bounds<Points>, String)>,
    unknown: Vec<String>,
    // The translation the current element sits at (only translations occur).
    tx: f32,
    ty: f32,
    // The area drawn inside each open `<a>`.
    link_box: Vec<(String, [f32; 4])>,
}

fn n(v: f32) -> Num {
    Num::Const(v)
}

fn num(s: Option<&str>) -> f32 {
    s.and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok()).unwrap_or(0.0)
}

impl Ctx<'_> {
    /// A CSS value with its `var()`s replaced: palette tokens, the root's
    /// aliases, else the fallback.
    fn resolve(&self, value: &str, depth: u8) -> String {
        let v = value.trim();
        if depth > 8 {
            return v.to_owned();
        }
        if let Some(inner) = v.strip_prefix("color-mix(in srgb,").and_then(|r| r.strip_suffix(')')) {
            return self.mix(inner, depth).unwrap_or_default();
        }
        match Paint::parse(v) {
            Paint::Var { name, fallback } => {
                let found = if let Some(token) = name.strip_prefix("pele-") {
                    self.palette.get(token).map(str::to_owned)
                } else {
                    self.aliases.iter().find(|(k, _)| k.strip_prefix("--") == Some(name.as_str())).map(|(_, v)| v.clone())
                };
                match found.or(fallback) {
                    Some(f) => self.resolve(&f, depth + 1),
                    None => String::new(),
                }
            }
            _ => v.to_owned(),
        }
    }

    /// `color-mix(in srgb,A p%,B)`.
    fn mix(&self, inner: &str, depth: u8) -> Option<String> {
        let (a, b) = split_top(inner)?;
        let a = a.trim();
        let (a, pct) = match a.rsplit_once(' ') {
            Some((c, p)) if p.ends_with('%') => (c, p.trim_end_matches('%').parse::<f32>().ok()? / 100.0),
            _ => (a, 0.5),
        };
        let ca = Color::parse(&self.resolve(a, depth + 1))?.to_f64();
        let cb = Color::parse(&self.resolve(b, depth + 1))?.to_f64();
        let p = pct as f64;
        let m = |x: f64, y: f64| ((x * p + y * (1.0 - p)) * 255.0).round() as u8;
        let c = Color::rgba(m(ca.0, cb.0), m(ca.1, cb.1), m(ca.2, cb.2), m(ca.3, cb.3));
        Some(format!("#{:08x}", c.0))
    }

    fn color(&self, value: &str, st: &State, opacity: f32) -> Option<Color> {
        let v = if value.trim() == "currentColor" { st.color.clone() } else { value.to_owned() };
        let r = self.resolve(&v, 0);
        if r.is_empty() || r == "none" || r == "transparent" {
            return None;
        }
        let c = Color::parse(&r)?;
        if opacity >= 1.0 {
            return Some(c);
        }
        let a = ((c.0 & 0xff) as f32 * opacity.clamp(0.0, 1.0)).round() as u32;
        Some(Color(c.0 & !0xff | a))
    }

    fn grow(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let (tx, ty) = (self.tx, self.ty);
        for (_, b) in self.link_box.iter_mut() {
            b[0] = b[0].min(x0 + tx);
            b[1] = b[1].min(y0 + ty);
            b[2] = b[2].max(x1 + tx);
            b[3] = b[3].max(y1 + ty);
        }
    }

    /// Fills and strokes the path `ops` as `st` says.
    fn paint(&mut self, ops: Vec<PathOp>, st: &State) {
        let fill = self.color(&st.fill, st, st.fill_opacity * st.opacity);
        let stroke = self.color(&st.stroke, st, st.stroke_opacity * st.opacity);
        if fill.is_none() && stroke.is_none() {
            return;
        }
        self.ops.push(CanvasOp::BeginPath);
        for op in ops {
            self.ops.push(CanvasOp::Path(op));
        }
        if let Some(c) = fill {
            self.ops.push(CanvasOp::FillStyle(CPaint::Color(c)));
            self.ops.push(CanvasOp::Fill(None, FillRule::NonZero));
        }
        if let Some(c) = stroke {
            self.ops.push(CanvasOp::StrokeStyle(CPaint::Color(c)));
            self.ops.push(CanvasOp::LineWidth(n(st.stroke_width)));
            self.ops.push(CanvasOp::LineCap(st.cap));
            self.ops.push(CanvasOp::LineJoin(st.join));
            let dash: Vec<Points> = st
                .dash
                .as_deref()
                .map(|d| d.split([' ', ',']).filter_map(|p| p.trim().trim_end_matches("px").parse::<f32>().ok()).map(px).collect())
                .unwrap_or_default();
            self.ops.push(CanvasOp::LineDash(dash.into()));
            self.ops.push(CanvasOp::Stroke(None));
        }
    }

    fn apply(&mut self, el: &Element, st: &mut State) {
        let mut props: Vec<(String, String)> = el.attrs.iter().map(|(k, v)| (k.clone(), unescape(v))).collect();
        // CSS (`style=`) wins over presentation attributes.
        if let Some(s) = el.attr("style") {
            props.extend(declarations(s));
        }
        for (k, v) in &props {
            if !ATTRS.contains(&k.as_str()) && !k.starts_with("--") && !CSS_PROPS.contains(&k.as_str()) {
                self.unknown.push(format!("{}@{}", el.name, k));
            }
            match k.as_str() {
                "fill" => st.fill = v.clone(),
                "stroke" => st.stroke = v.clone(),
                "stroke-width" => st.stroke_width = num(Some(&self.resolve(v, 0))),
                "fill-opacity" => st.fill_opacity = num(Some(v)),
                "stroke-opacity" => st.stroke_opacity = num(Some(v)),
                "opacity" => st.opacity *= num(Some(v)),
                "text-decoration" => st.underline = v.contains("underline"),
                "stroke-dasharray" => st.dash = Some(v.clone()),
                "stroke-linecap" => st.cap = if v == "round" { LineCap::Round } else if v == "square" { LineCap::Square } else { LineCap::Butt },
                "stroke-linejoin" => st.join = if v == "round" { LineJoin::Round } else if v == "bevel" { LineJoin::Bevel } else { LineJoin::Miter },
                "font-size" => st.font_size = if v == "smaller" { st.font_size / 1.2 } else { num(Some(v)) },
                "font-weight" => st.weight = v.clone(),
                "font-style" => st.italic = v == "italic",
                "font-family" => st.family = v.clone(),
                "text-anchor" => st.anchor = v.clone(),
                "color" => st.color = v.clone(),
                "rx" => st.rx = Some(v.clone()),
                "font" => {
                    if let Some(size) = v.split_whitespace().find(|p| p.ends_with("px")) {
                        st.font_size = num(Some(size));
                    }
                }
                _ => {}
            }
        }
    }

    fn font(&self, st: &State) -> CanvasFont {
        let weight = self.resolve(&st.weight, 0);
        let weight = match weight.as_str() {
            "bold" | "bolder" => "700".to_owned(),
            "normal" | "" => "400".to_owned(),
            w => w.to_owned(),
        };
        let family = self.resolve(&st.family, 0);
        let spec = format!("{}{} {}px {}", if st.italic { "italic " } else { "" }, weight, st.font_size, if family.is_empty() { "sans-serif" } else { &family });
        CanvasFont::parse(&spec).unwrap_or_default()
    }

    fn text(&mut self, el: &Element, st: &State) {
        // A `<text>` is a label: its lines are `<tspan>`s with x/y, or the
        // text itself at the element's x/y.
        let label: String = flat_text(el);
        let mut lines: Vec<(f32, f32, String, Vec<(String, State)>)> = Vec::new();
        let x = num(el.attr("x"));
        let y = num(el.attr("y"));
        for child in &el.children {
            match child {
                Node::Text(t) => lines.push((x, y, st.anchor.clone(), vec![(unescape(t), st.clone())])),
                Node::Element(span) if span.name == "tspan" => {
                    let mut sst = st.clone();
                    self.apply(span, &mut sst);
                    let text = flat_text(span);
                    match (span.attr("x"), span.attr("y")) {
                        (Some(sx), Some(sy)) => lines.push((num(Some(sx)), num(Some(sy)), sst.anchor.clone(), vec![(text, sst)])),
                        _ => match lines.last_mut() {
                            Some(line) => line.3.push((text, sst)),
                            None => lines.push((x, y, st.anchor.clone(), vec![(text, sst)])),
                        },
                    }
                }
                Node::Element(other) => self.unknown.push(format!("text>{}", other.name)),
            }
        }
        let size = st.font_size;
        for (lx, ly, anchor, spans) in lines {
            // Several spans on a line are set side by side from Pele's own
            // widths, the line centered as a whole as SVG does.
            let widths: Vec<f32> = spans.iter().map(|(t, s)| pele::text::measurer::width(t, s.font_size as f64, style_bits(s, self)) as f32).collect();
            let total: f32 = widths.iter().sum();
            let single = spans.len() == 1;
            let mut x = match anchor.as_str() {
                "middle" if !single => lx - total / 2.0,
                "end" if !single => lx - total,
                _ => lx,
            };
            let align = if !single {
                CanvasTextAlign::Left
            } else {
                match anchor.as_str() {
                    "middle" => CanvasTextAlign::Center,
                    "end" => CanvasTextAlign::Right,
                    _ => CanvasTextAlign::Left,
                }
            };
            for ((text, sst), w) in spans.iter().zip(&widths) {
                let fill = self.color(&sst.fill, sst, sst.fill_opacity * sst.opacity);
                let stroke = self.color(&sst.stroke, sst, sst.stroke_opacity * sst.opacity);
                self.ops.push(CanvasOp::Font(self.font(sst)));
                self.ops.push(CanvasOp::TextAlign(align));
                self.ops.push(CanvasOp::TextBaseline(CanvasTextBaseline::Alphabetic));
                let t: Arc<str> = text.as_str().into();
                if let Some(c) = stroke {
                    self.ops.push(CanvasOp::StrokeStyle(CPaint::Color(c)));
                    self.ops.push(CanvasOp::LineWidth(n(sst.stroke_width)));
                    self.ops.push(CanvasOp::LineJoin(sst.join));
                    self.ops.push(CanvasOp::StrokeText { text: t.clone(), x: n(x), y: n(ly), max_width: None });
                }
                if let Some(c) = fill {
                    self.ops.push(CanvasOp::FillStyle(CPaint::Color(c)));
                    self.ops.push(CanvasOp::FillText { text: t, x: n(x), y: n(ly), max_width: None });
                    if sst.underline {
                        // A rule under the span, as a browser draws `text-decoration: underline`.
                        let left = match align {
                            CanvasTextAlign::Center => x - w / 2.0,
                            CanvasTextAlign::Right => x - w,
                            _ => x,
                        };
                        let thick = (sst.font_size / 16.0).max(1.0);
                        self.ops.push(CanvasOp::FillRect([n(left), n(ly + sst.font_size * 0.12), n(*w), n(thick)]));
                    }
                }
                x += w;
            }
            let left = match anchor.as_str() {
                "middle" => lx - total / 2.0,
                "end" => lx - total,
                _ => lx,
            };
            self.grow(left, ly - size, left + total, ly + size * 0.3);
        }
        if !label.trim().is_empty() {
            let b = Bounds::new(point(px(x - 1.0 + self.tx), px(y - size + self.ty)), nsh_core::prelude::size(px(2.0), px(size * 1.3)));
            self.nodes.push(CanvasNode::new(b, AccessibilityRole::Text, label.trim().to_owned()));
        }
    }

    fn element(&mut self, el: &Element, parent: &State) {
        if !ELEMENTS.contains(&el.name.as_str()) {
            self.unknown.push(el.name.clone());
            return;
        }
        let mut st = parent.clone();
        self.apply(el, &mut st);
        match el.name.as_str() {
            "title" | "desc" | "image" => {}
            "text" => self.text(el, &st),
            "path" => {
                let ops = parse_svg_path(el.attr("d").unwrap_or(""));
                self.grow_ops(&ops);
                self.paint(ops, &st);
            }
            "rect" => {
                let (x, y, w, h) = (num(el.attr("x")), num(el.attr("y")), num(el.attr("width")), num(el.attr("height")));
                let rx = match el.attr("rx") {
                    Some(r) => num(Some(r)),
                    None => st.rx.as_deref().map_or(0.0, |r| num(Some(&self.resolve(r, 0)))),
                };
                let op = if rx > 0.0 { PathOp::RoundRect([n(x), n(y), n(w), n(h)], [n(rx); 4]) } else { PathOp::Rect([n(x), n(y), n(w), n(h)]) };
                self.grow(x, y, x + w, y + h);
                self.paint(vec![op], &st);
            }
            "circle" | "ellipse" => {
                let (cx, cy) = (num(el.attr("cx")), num(el.attr("cy")));
                let (rx, ry) = if el.name == "circle" { (num(el.attr("r")), num(el.attr("r"))) } else { (num(el.attr("rx")), num(el.attr("ry"))) };
                self.grow(cx - rx, cy - ry, cx + rx, cy + ry);
                self.paint(vec![PathOp::Ellipse { geometry: [n(cx), n(cy), n(rx), n(ry), n(0.0)], start: n(0.0), end: n(TAU), ccw: false }], &st);
            }
            "polygon" => {
                let pts: Vec<f32> = el.attr("points").unwrap_or("").split([' ', ',']).filter_map(|p| p.parse().ok()).collect();
                let mut ops = Vec::new();
                for (i, p) in pts.chunks(2).enumerate() {
                    if p.len() < 2 {
                        break;
                    }
                    self.grow(p[0], p[1], p[0], p[1]);
                    ops.push(if i == 0 { PathOp::MoveTo(n(p[0]), n(p[1])) } else { PathOp::LineTo(n(p[0]), n(p[1])) });
                }
                ops.push(PathOp::Close);
                self.paint(ops, &st);
            }
            _ => {
                // svg, g, a: groups.
                let translate = el.attr("transform").and_then(parse_translate);
                if let Some((x, y)) = translate {
                    self.ops.push(CanvasOp::Save);
                    self.ops.push(CanvasOp::Translate(n(x), n(y)));
                    self.tx += x;
                    self.ty += y;
                }
                let href = if el.name == "a" { el.attr("href").map(unescape) } else { None };
                if let Some(h) = &href {
                    self.link_box.push((h.clone(), [f32::MAX, f32::MAX, f32::MIN, f32::MIN]));
                }
                for child in &el.children {
                    if let Node::Element(c) = child {
                        self.element(c, &st);
                    }
                }
                if href.is_some() {
                    let (h, b) = self.link_box.pop().unwrap();
                    if b[0] <= b[2] {
                        self.links.push((Bounds::new(point(px(b[0]), px(b[1])), nsh_core::prelude::size(px(b[2] - b[0]), px(b[3] - b[1]))), h));
                    }
                }
                if let Some((x, y)) = translate {
                    self.ops.push(CanvasOp::Restore);
                    self.tx -= x;
                    self.ty -= y;
                }
            }
        }
    }

    fn grow_ops(&mut self, ops: &[PathOp]) {
        if self.link_box.is_empty() {
            return;
        }
        for op in ops {
            let pts: &[Num] = match op {
                PathOp::MoveTo(x, y) | PathOp::LineTo(x, y) => &[*x, *y][..],
                PathOp::CubicTo(p) => &p[4..6],
                PathOp::QuadTo(p) => &p[2..4],
                _ => &[],
            };
            if let [Num::Const(x), Num::Const(y)] = pts {
                let (x, y) = (*x, *y);
                self.grow(x, y, x, y);
            }
        }
    }
}

const CSS_PROPS: &[&str] = &[
    "max-width", "font", "letter-spacing", "word-spacing", "text-transform", "direction", "stroke-opacity", "stroke-dashoffset",
    "opacity",
];

fn style_bits(st: &State, ctx: &Ctx) -> u32 {
    use pele::text::measurer::style;
    let w = ctx.resolve(&st.weight, 0);
    let bold = matches!(w.as_str(), "bold" | "bolder") || w.parse::<f32>().is_ok_and(|n| n >= 600.0);
    (if bold { style::BOLD } else { 0 }) | (if st.italic { style::ITALIC } else { 0 }) | (if st.family.contains("--_fm") { style::MONO } else { 0 })
}

fn flat_text(el: &Element) -> String {
    let mut out = String::new();
    for c in &el.children {
        match c {
            Node::Text(t) => out.push_str(&unescape(t)),
            Node::Element(e) => out.push_str(&flat_text(e)),
        }
    }
    out
}

fn split_top(s: &str) -> Option<(&str, &str)> {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => return Some((&s[..i], &s[i + 1..])),
            _ => {}
        }
    }
    None
}

fn parse_translate(t: &str) -> Option<(f32, f32)> {
    let inner = t.trim().strip_prefix("translate(")?.strip_suffix(')')?;
    let (x, y) = inner.split_once(',').unwrap_or((inner, "0"));
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// The drawing for `scene` in `palette`.
pub(crate) fn draw(scene: &Scene, palette: &PelePalette) -> Drawn {
    let mut ctx = Ctx {
        palette,
        aliases: scene.root_style(),
        ops: Vec::new(),
        nodes: Vec::new(),
        links: Vec::new(),
        unknown: Vec::new(),
        tx: 0.0,
        ty: 0.0,
        link_box: Vec::new(),
    };
    let family = ctx.aliases.iter().find(|(k, _)| k == "font-family").map(|(_, v)| v.clone()).unwrap_or_else(|| "sans-serif".into());
    let root = State {
        fill: "black".into(),
        stroke: "none".into(),
        stroke_width: 1.0,
        fill_opacity: 1.0,
        stroke_opacity: 1.0,
        opacity: 1.0,
        underline: false,
        dash: None,
        cap: LineCap::Butt,
        join: LineJoin::Miter,
        font_size: 16.0,
        weight: "normal".into(),
        italic: false,
        family,
        anchor: "start".into(),
        color: "var(--_fg)".into(),
        rx: None,
    };
    ctx.element(&scene.root, &root);
    Drawn {
        drawing: Arc::new(Drawing { ops: ctx.ops, nodes: ctx.nodes, ..Default::default() }),
        w: scene.width() as f32,
        h: scene.height() as f32,
        links: ctx.links,
        unknown: ctx.unknown,
    }
}
