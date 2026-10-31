//! A laid-out page on an nsh canvas: each display-list primitive becomes
//! the canvas operations BetterOffice's own painters issue (its tiny-skia
//! rasterizer, `docx-raster`, is the reference for the paint order:
//! background, body, notes, header and footer, page borders).
//!
//! Text comes shaped: a glyph run places every glyph. The canvas draws text
//! by font and string, so a run is drawn a word at a time, each word at its
//! first glyph's place: the shaper and CoreText read the same face
//! ([`super::fonts`]), and a justified line's stretched spaces land where
//! the layout put them.
//!
//! Pictures are not drawn here: the page lists them ([`Picture`]) and the
//! view shows each as an image view over the canvas.
//!
//! Tracked changes and comments are shown in their author's color:
//! insertions underlined, deletions struck through, commented text tinted.

use std::collections::HashMap;
use std::str::FromStr;

use docx_layout::display_list::{DecoKind, DisplayPage, GlyphRunPrimitive, LinePrimitive, Primitive, RevisionKind, ShapePathCommand, TextRunPrimitive};
use nsh_core::canvas::{CanvasTextAlign, CanvasTextBaseline, Drawing};
use nsh_core::prelude::*;
use serde_json::Number;

use super::fonts::{Face, Fonts};

/// A picture on a page, in page pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    /// The relationship that names it, and the part that owns the id (the
    /// body's, or a header's or footer's).
    pub rel_id: String,
    pub part: Option<String>,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub opacity: f32,
}

/// A page drawn: its canvas drawing and the pictures over it.
#[derive(Clone, Debug, Default)]
pub struct Drawn {
    pub w: f32,
    pub h: f32,
    pub drawing: Drawing,
    pub pictures: Vec<Picture>,
}

fn num(n: &Number) -> f32 {
    n.as_f64().unwrap_or(0.0) as f32
}

/// A CSS color; `None` for none or transparent.
fn color(s: &str) -> Option<Hsla> {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("transparent") {
        return None;
    }
    let s = if !s.starts_with('#') && s.len() == 6 && s.chars().all(|c| c.is_ascii_hexdigit()) { format!("#{s}") } else { s.to_owned() };
    Hsla::from_str(&s).ok()
}

/// An author's color: one of a few that read on white, by name.
pub fn author_color(author: &str) -> Hsla {
    const COLORS: [&str; 8] = ["#c0392b", "#2471a3", "#1e8449", "#8e44ad", "#d35400", "#117a65", "#b7950b", "#a93226"];
    let h = author.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    Hsla::from_str(COLORS[h as usize % COLORS.len()]).unwrap_or_default()
}

/// The `family`, weight and slant of a CSS font shorthand
/// (`italic bold 14.67px "Calibri", sans-serif`).
fn parse_css_font(font: &str) -> (String, bool, bool, f64) {
    let mut bold = false;
    let mut italic = false;
    let mut size = 14.67;
    let mut rest = font;
    for (i, word) in font.split_whitespace().enumerate() {
        match word {
            "italic" | "oblique" => italic = true,
            "bold" | "bolder" | "600" | "700" | "800" | "900" => bold = true,
            w if w.ends_with("px") && w[..w.len() - 2].parse::<f64>().is_ok() => {
                size = w[..w.len() - 2].parse().unwrap_or(size);
                let at = font.split_whitespace().take(i + 1).map(|w| w.len() + 1).sum::<usize>();
                rest = font.get(at.min(font.len())..).unwrap_or("");
                break;
            }
            _ => {}
        }
    }
    let family = rest.split(',').next().unwrap_or("").trim().trim_matches(['"', '\'']).to_owned();
    (family, bold, italic, size)
}

/// The canvas state set so far, so a run of words sets each only when it
/// changes (a page is thousands of words).
#[derive(Default)]
struct Pen {
    font: Option<String>,
    fill: Option<Hsla>,
}

impl Pen {
    fn fill(&mut self, p: Painter, c: Hsla) -> Painter {
        if self.fill == Some(c) {
            return p;
        }
        self.fill = Some(c);
        p.fill_color(c)
    }

    fn font(&mut self, p: Painter, font: &str) -> Painter {
        if self.font.as_deref() == Some(font) {
            return p;
        }
        self.font = Some(font.to_owned());
        p.font(font)
    }

    /// After a save/restore pair everything is unknown again.
    fn forget(&mut self) {
        *self = Pen::default();
    }
}

struct Ctx<'a> {
    fonts: &'a Fonts,
    /// CSS fonts of text runs, resolved once per page.
    css: HashMap<String, (Face, f64)>,
    pen: Pen,
    pictures: Vec<Picture>,
    part: Option<String>,
}

/// Draws `page` at `zoom` with `fonts` (the faces the layout measured with).
pub fn page(page: &DisplayPage, fonts: &Fonts, zoom: f32) -> Drawn {
    let (w, h) = (num(&page.width), num(&page.height));
    let mut cx = Ctx { fonts, css: HashMap::new(), pen: Pen::default(), pictures: Vec::new(), part: None };
    let mut p = Painter::new().text_baseline(CanvasTextBaseline::Alphabetic).text_align(CanvasTextAlign::Left);
    if zoom != 1.0 {
        p = p.scale(zoom, zoom);
    }
    let bg = page.background.as_deref().and_then(color).unwrap_or(Hsla { h: 0.0, s: 0.0, l: 1.0, a: 1.0 });
    p = cx.pen.fill(p, bg).fill_rect(0.0, 0.0, w, h);
    for prim in &page.primitives {
        p = primitive(p, prim, &mut cx);
    }
    for area in &page.note_areas {
        cx.part = Some(format!("notes:{}", area.kind.as_deref().unwrap_or("footnote")));
        for prim in area.separator_primitives.iter().chain(&area.primitives) {
            p = primitive(p, prim, &mut cx);
        }
    }
    for region in [&page.header, &page.footer].into_iter().flatten() {
        cx.part = Some(region.r_id.clone());
        for prim in &region.primitives {
            p = primitive(p, prim, &mut cx);
        }
    }
    for b in &page.page_borders {
        let (x, y, bw, bh) = (num(&b.x), num(&b.y), num(&b.w), num(&b.h));
        for (side, seg) in [(&b.top, (x, y, x + bw, y)), (&b.right, (x + bw, y, x + bw, y + bh)), (&b.bottom, (x, y + bh, x + bw, y + bh)), (&b.left, (x, y, x, y + bh))] {
            if let Some(s) = side
                && let Some(c) = color(&s.color)
            {
                p = p.begin_path().move_to(seg.0, seg.1).line_to(seg.2, seg.3).stroke_color(c).line_width(num(&s.width).max(0.5)).line_dash(&[]).stroke();
            }
        }
    }
    Drawn { w: w * zoom, h: h * zoom, drawing: p.finish(), pictures: cx.pictures }
}

fn primitive(mut p: Painter, prim: &Primitive, cx: &mut Ctx) -> Painter {
    match prim {
        Primitive::Rect(r) => {
            if let Some(c) = color(&r.fill) {
                let opacity = r.attrs.primitive_opacity.as_ref().map_or(1.0, num);
                let mut c = c;
                c.a *= opacity;
                p = cx.pen.fill(p, c).fill_rect(num(&r.x), num(&r.y), num(&r.w), num(&r.h));
            }
            p
        }
        Primitive::Line(l) => line(p, l),
        Primitive::Decoration(d) => {
            // a tracked change's or comment's mark takes its author's color
            let author = d.attrs.revision.as_ref().map(|r| r.author.as_str()).or(d.attrs.comment.as_ref().and_then(|c| c.author_name.as_deref()));
            let Some(mut c) = author.map(author_color).or_else(|| color(&d.color)) else { return p };
            let (x, y, w, h) = (num(&d.x), num(&d.y), num(&d.w), num(&d.h));
            match d.deco {
                DecoKind::Highlight | DecoKind::CommentRange => {
                    if d.deco == DecoKind::CommentRange {
                        c.a = c.a.min(0.25);
                    }
                    cx.pen.fill(p, c).fill_rect(x, y, w, h)
                }
                DecoKind::Spell => p,
                DecoKind::Underline | DecoKind::Strike if d.dashed || d.dotted => {
                    let dash = if d.dotted { [px(1.0), px(1.5)] } else { [px(3.0), px(2.0)] };
                    let mid = y + h / 2.0;
                    p.begin_path().move_to(x, mid).line_to(x + w, mid).stroke_color(c).line_width(h.max(0.75)).line_dash(&dash).stroke().line_dash(&[])
                }
                _ => cx.pen.fill(p, c).fill_rect(x, y, w, h.max(0.75)),
            }
        }
        Primitive::Text(t) => text_run(p, t, cx),
        Primitive::GlyphRun(g) => glyph_run(p, g, cx),
        Primitive::Image(i) => {
            cx.pictures.push(Picture {
                rel_id: i.rel_id.clone(),
                part: cx.part.clone(),
                x: num(&i.x),
                y: num(&i.y),
                w: num(&i.w),
                h: num(&i.h),
                opacity: i.opacity.as_ref().map_or(1.0, num),
            });
            p
        }
        Primitive::Shape(s) => {
            let (x, y, w, h) = (num(&s.x), num(&s.y), num(&s.w), num(&s.h));
            let transformed = s.transform.as_ref().is_some_and(|t| t.rotation.is_some() || t.flip_h || t.flip_v);
            if transformed {
                let t = s.transform.as_ref().expect("checked");
                let (cx0, cy0) = (x + w / 2.0, y + h / 2.0);
                p = p.save().translate(cx0, cy0);
                if let Some(r) = &t.rotation {
                    p = p.rotate(num(r).to_radians());
                }
                p = p.scale(if t.flip_h { -1.0 } else { 1.0 }, if t.flip_v { -1.0 } else { 1.0 }).translate(-cx0, -cy0);
                cx.pen.forget();
            }
            p = p.begin_path();
            for c in &s.geometry_path {
                p = match c {
                    ShapePathCommand::Move { x, y } => p.move_to(num(x), num(y)),
                    ShapePathCommand::Line { x, y } => p.line_to(num(x), num(y)),
                    ShapePathCommand::Quad { cpx, cpy, x, y } => p.quadratic_curve_to(point(px(num(cpx)), px(num(cpy))), point(px(num(x)), px(num(y)))),
                    ShapePathCommand::Cubic { cp1x, cp1y, cp2x, cp2y, x, y } => {
                        p.bezier_curve_to(point(px(num(cp1x)), px(num(cp1y))), point(px(num(cp2x)), px(num(cp2y))), point(px(num(x)), px(num(y))))
                    }
                    ShapePathCommand::Close => p.close_path(),
                };
            }
            if let Some(c) = s.fill.as_deref().and_then(color) {
                p = cx.pen.fill(p, c).fill();
            }
            if let Some(st) = &s.stroke
                && let Some(c) = color(&st.color)
            {
                p = p.stroke_color(c).line_width(num(&st.width).max(0.25)).line_dash(&[]).stroke();
            }
            if transformed {
                p = p.restore();
                cx.pen.forget();
            }
            p
        }
    }
}

fn line(p: Painter, l: &LinePrimitive) -> Painter {
    let Some(mut c) = color(&l.color) else { return p };
    c.a *= l.opacity.as_ref().map_or(1.0, num);
    let dash: Vec<Points> = l.dash.as_ref().map(|d| d.iter().map(|n| px(num(n))).collect()).unwrap_or_default();
    p.begin_path()
        .move_to(num(&l.x1), num(&l.y1))
        .line_to(num(&l.x2), num(&l.y2))
        .stroke_color(c)
        .line_width(num(&l.stroke_width).max(0.25))
        .line_dash(&dash)
        .stroke()
}

/// The color a run's text takes: an author's for a tracked change.
fn ink(own: &str, attrs: &docx_layout::display_list::DocAttrs) -> (Option<Hsla>, Option<RevisionKind>) {
    if let Some(r) = &attrs.revision {
        return (Some(author_color(&r.author)), Some(r.kind));
    }
    (color(own), None)
}

/// A tracked change's mark under or through a word.
fn change_mark(p: Painter, kind: Option<RevisionKind>, c: Hsla, x: f32, baseline: f32, w: f32, size: f32) -> Painter {
    let t = (size / 14.0).max(1.0);
    match kind {
        Some(RevisionKind::Ins) => p.fill_rect(x, baseline + size * 0.12, w, t),
        Some(RevisionKind::Del) => p.fill_rect(x, baseline - size * 0.3, w, t),
        None => {
            let _ = c;
            p
        }
    }
}

fn glyph_run(mut p: Painter, g: &GlyphRunPrimitive, cx: &mut Ctx) -> Painter {
    if g.hidden || g.glyphs.is_empty() {
        return p;
    }
    let (Some(c), kind) = ink(&g.color, &g.attrs) else { return p };
    let face = cx.fonts.face(g.font_id).cloned().unwrap_or_else(|| Fonts::resolve("Arial", false, false));
    let size = g.size as f32;
    p = cx.pen.font(p, &face.css(g.size));
    p = cx.pen.fill(p, c);
    let alpha = g.opacity.as_ref().map(num).filter(|o| *o < 1.0);
    if let Some(a) = alpha {
        p = p.global_alpha(a);
    }
    let text = &g.text;
    let caps = |s: &str| if g.all_caps || g.small_caps { s.to_uppercase() } else { s.to_owned() };
    let rtl = g.rtl == Some(true) || g.glyphs.iter().any(|gl| gl.bidi_level.is_some_and(|l| l % 2 == 1));
    if rtl {
        // right to left: CoreText orders it; the run starts at its left edge
        let x0 = g.glyphs.iter().map(|gl| gl.x).fold(f64::MAX, f64::min) as f32;
        let x1 = g.glyphs.iter().map(|gl| gl.x + gl.advance).fold(f64::MIN, f64::max) as f32;
        let y = g.glyphs[0].y as f32;
        p = p.fill_text(&caps(text.trim()), x0, y);
        p = change_mark(p, kind, c, x0, y, x1 - x0, size);
    } else {
        // words: break after each space cluster
        let n = g.glyphs.len();
        let mut start = 0;
        for i in 0..n {
            let at = g.glyphs[i].cluster as usize;
            let end = if i + 1 < n { g.glyphs[i + 1].cluster as usize } else { text.len() };
            let space = text.get(at..end.max(at)).is_some_and(|s| s.chars().all(char::is_whitespace));
            if !(space || i + 1 == n) {
                continue;
            }
            let (a, b) = (g.glyphs[start].cluster as usize, end);
            if let Some(word) = text.get(a.min(b)..b.max(a)) {
                let word = word.trim_end();
                let (gx, gy) = (g.glyphs[start].x as f32, g.glyphs[start].y as f32);
                if !word.is_empty() {
                    p = p.fill_text(&caps(word), gx, gy);
                }
                if kind.is_some() {
                    let right = (g.glyphs[i].x + g.glyphs[i].advance) as f32;
                    p = change_mark(p, kind, c, gx, gy, right - gx, size);
                }
            }
            start = i + 1;
        }
    }
    if alpha.is_some() {
        p = p.global_alpha(1.0);
    }
    p
}

fn text_run(mut p: Painter, t: &TextRunPrimitive, cx: &mut Ctx) -> Painter {
    if t.hidden || t.text.trim().is_empty() {
        return p;
    }
    let (Some(c), kind) = ink(&t.color, &t.attrs) else { return p };
    let (face, size) = cx
        .css
        .entry(t.font.clone())
        .or_insert_with(|| {
            let (family, bold, italic, size) = parse_css_font(&t.font);
            (Fonts::resolve(&family, bold, italic), size)
        })
        .clone();
    p = cx.pen.font(p, &face.css(size));
    p = cx.pen.fill(p, c);
    let text = if t.all_caps || t.small_caps { t.text.to_uppercase() } else { t.text.clone() };
    let (x, y) = (num(&t.x), num(&t.baseline_y));
    p = p.fill_text(&text, x, y);
    change_mark(p, kind, c, x, y, num(&t.width), size as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_fonts_parse() {
        assert_eq!(parse_css_font(r#"italic bold 14.67px "Calibri", sans-serif"#), ("Calibri".to_owned(), true, true, 14.67));
        assert_eq!(parse_css_font("16px Georgia"), ("Georgia".to_owned(), false, false, 16.0));
    }

    #[test]
    fn colors_read_with_or_without_a_hash() {
        assert!(color("transparent").is_none());
        assert_eq!(color("ff0000"), color("#ff0000"));
        assert!(color("#1F3864").is_some());
        assert_ne!(author_color("Ada"), author_color("Grace"));
    }
}
