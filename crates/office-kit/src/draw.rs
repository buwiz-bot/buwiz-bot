//! BetterOffice display lists on an nsh canvas: each `DrawCmd` becomes
//! the Canvas 2D operations BetterOffice's own browser painter issues
//! (its tiny-skia twin, `betteroffice-xlsx-raster`, is the reference
//! here). Coordinates stay the list's (viewport pixels = points).
//!
//! The document's colors are its own, except in dark mode: the default
//! white sheet, its gridlines and the automatic black text take the Rox
//! theme's colors, as Excel and Numbers do, while fills and colors the
//! author chose stay. Text over an explicit fill keeps black.

use std::str::FromStr;

use betteroffice_xlsx::{Align, DisplayList, DrawCmd, Rect};
use nsh_core::canvas::{CanvasTextAlign, CanvasTextBaseline, Drawing};
use nsh_core::prelude::*;
use xlsx_render::GeometryPathCommand;

/// BetterOffice's defaults (`xlsx-render`), recolored in dark mode.
const SHEET_BG: &str = "#ffffff";
const GRIDLINE: &str = "#d4d4d4";
const AUTO_TEXT: &str = "#000000";

/// The colors the default sheet takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub bg: Hsla,
    pub grid: Hsla,
    pub text: Hsla,
}

impl Palette {
    /// The document's own colors.
    pub fn light() -> Self {
        Palette { dark: false, bg: hex(SHEET_BG), grid: hex(GRIDLINE), text: hex(AUTO_TEXT) }
    }

    pub fn dark() -> Self {
        Palette { dark: true, bg: hex("#1e1e1e"), grid: hex("#3a3a3a"), text: hex("#e8e8e8") }
    }
}

/// `#rrggbb` (or `#rrggbbaa`) as a color; black when unreadable.
pub fn hex(s: &str) -> Hsla {
    Hsla::from_str(s).unwrap_or_else(|_| Hsla { h: 0.0, s: 0.0, l: 0.0, a: 1.0 })
}

/// Measures `text` in a CSS `font` (the canvas's `measureText`).
pub type Measure<'a> = &'a dyn Fn(&str, &str) -> f32;

/// The CSS font of a cell's text: BetterOffice gives points. A family
/// with a bundled metric-compatible face is drawn, and measured (the
/// `#####` rule, autofit), in that face: Calibri as Carlito.
pub fn css_font(size_pt: f32, bold: bool, italic: bool, family: Option<&str>) -> String {
    let px = size_pt * 4.0 / 3.0;
    let family = family.filter(|f| !f.is_empty()).map(|f| format!("\"{}\"", super::fonts::canvas_family(f).replace('"', ""))).unwrap_or_else(|| "system-ui".to_owned());
    format!("{}{}{px}px {family}", if italic { "italic " } else { "" }, if bold { "bold " } else { "" })
}

fn bounds(r: &Rect) -> Bounds<Points> {
    Bounds { origin: point(px(r.x), px(r.y)), size: size(px(r.w.max(0.0)), px(r.h.max(0.0))) }
}

fn contains(r: &Rect, x: f32, y: f32) -> bool {
    x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
}

/// The share of an 8×8 pattern tile its foreground covers: a canvas has no
/// pattern paint here, so a patterned fill paints its foreground at that
/// opacity over its background (a 50% gray reads as 50% gray).
fn coverage(mask: [u8; 8]) -> f32 {
    mask.iter().map(|b| b.count_ones()).sum::<u32>() as f32 / 64.0
}

/// The canvas state the drawing has set, so a run of cells sets each
/// only when it changes (a 60×40 window is thousands of texts): the clip
/// in force (`save` + `clip`, `restore` when it changes) and the font,
/// alignment and fill since.
#[derive(Default)]
struct Pen {
    clip: Option<(f32, f32, f32, f32)>,
    font: Option<String>,
    align: Option<CanvasTextAlign>,
    fill: Option<Hsla>,
    alphabetic: bool,
}

impl Pen {
    fn clip(&mut self, p: Painter, clip: Option<&Rect>) -> Painter {
        let want = clip.map(|r| (r.x, r.y, r.w, r.h));
        if want == self.clip {
            return p;
        }
        let p = if self.clip.is_some() { p.restore() } else { p };
        // a restore brings back the state from before the save
        *self = Pen { clip: want, ..Pen::default() };
        match clip {
            Some(r) => p.save().begin_path().rect(bounds(r)).clip(),
            None => p,
        }
    }

    fn fill(&mut self, p: Painter, c: Hsla) -> Painter {
        if self.fill == Some(c) {
            return p;
        }
        self.fill = Some(c);
        p.fill_color(c)
    }

    fn font(&mut self, p: Painter, font: &str) -> Painter {
        let p = if self.alphabetic { p } else { p.text_baseline(CanvasTextBaseline::Alphabetic) };
        self.alphabetic = true;
        if self.font.as_deref() == Some(font) {
            return p;
        }
        self.font = Some(font.to_owned());
        p.font(font)
    }

    fn align(&mut self, p: Painter, a: CanvasTextAlign) -> Painter {
        if self.align == Some(a) {
            return p;
        }
        self.align = Some(a);
        p.text_align(a)
    }
}

/// Draws `list` into `p` in `palette`, measuring text with `measure`.
pub fn paint(mut p: Painter, list: &DisplayList, palette: &Palette, measure: Measure) -> Painter {
    // what an author filled: text over it keeps its own color
    let mut filled: Vec<Rect> = Vec::new();
    let mut pen = Pen::default();
    for (i, cmd) in list.commands.iter().enumerate() {
        match cmd {
            DrawCmd::FillRect { x, y, w, h, color, clip } => {
                let r = Rect { x: *x, y: *y, w: *w, h: *h };
                // the sheet's own background comes first and covers it
                let background = i == 0 && &**color == SHEET_BG && *x <= 0.0 && *y <= 0.0;
                let c = if background { palette.bg } else { hex(color) };
                if !background && palette.dark {
                    filled.push(clip.map_or(r, |c| intersect(&r, &c)));
                }
                p = pen.clip(p, clip.as_ref());
                p = pen.fill(p, c).fill_rect(*x, *y, *w, *h);
            }
            DrawCmd::PatternFill { x, y, w, h, pattern, fg, bg, clip, .. } => {
                let r = Rect { x: *x, y: *y, w: *w, h: *h };
                if palette.dark {
                    filled.push(clip.map_or(r, |c| intersect(&r, &c)));
                }
                p = pen.clip(p, clip.as_ref());
                p = pen.fill(p, hex(bg)).fill_rect(*x, *y, *w, *h);
                let mut f = hex(fg);
                f.a *= coverage(pattern.mask());
                p = pen.fill(p, f).fill_rect(*x, *y, *w, *h);
            }
            DrawCmd::Line { x1, y1, x2, y2, width, color, style, rail, clip } => {
                let grid = &**color == GRIDLINE && style.is_none();
                let c = if grid { palette.grid } else { hex(color) };
                p = pen.clip(p, clip.as_ref());
                p = line(p, (*x1, *y1, *x2, *y2), *width, c, style.as_deref(), *rail);
            }
            DrawCmd::Path { commands, fill, stroke, clip } => {
                p = pen.clip(p, clip.as_ref());
                p = p.begin_path();
                for c in commands {
                    p = match *c {
                        GeometryPathCommand::Move { x, y } => p.move_to(x as f32, y as f32),
                        GeometryPathCommand::Line { x, y } => p.line_to(x as f32, y as f32),
                        GeometryPathCommand::Quad { cpx, cpy, x, y } => p.quadratic_curve_to(point(px(cpx as f32), px(cpy as f32)), point(px(x as f32), px(y as f32))),
                        GeometryPathCommand::Cubic { cp1x, cp1y, cp2x, cp2y, x, y } => {
                            p.bezier_curve_to(point(px(cp1x as f32), px(cp1y as f32)), point(px(cp2x as f32), px(cp2y as f32)), point(px(x as f32), px(y as f32)))
                        }
                        GeometryPathCommand::Close => p.close_path(),
                    };
                }
                p = pen.fill(p, hex(fill)).fill();
                if let Some(s) = stroke {
                    p = p.stroke_color(hex(&s.color)).line_width(s.width).line_dash(&[]).stroke();
                }
            }
            DrawCmd::Text { x, y, text, font_size, color, clip, align, bold, italic, underline, strike, highlight, font_family, number_format, rotation_deg, wrap, .. } => {
                if text.is_empty() || clip.w <= 0.0 || clip.h <= 0.0 {
                    continue;
                }
                let over_fill = filled.iter().any(|f| contains(f, clip.x + clip.w / 2.0, clip.y + clip.h / 2.0));
                let c = if palette.dark && &**color == AUTO_TEXT && !over_fill { palette.text } else { hex(color) };
                let font = css_font(*font_size, *bold, *italic, font_family.as_deref());
                let px_size = *font_size * 4.0 / 3.0;
                let deco = TextDeco { underline: *underline, strike: *strike, size: px_size, color: c };
                // the font, color and alignment are set outside the text's
                // clip (a save/restore around it keeps them for the next)
                p = pen.clip(p, None);
                p = pen.font(p, &font);
                p = pen.fill(p, c);
                if let Some(deg) = rotation_deg {
                    // rotated about the cell's middle, as BetterOffice anchors it
                    p = p.save().begin_path().rect(bounds(clip)).clip().translate(*x, *y).rotate(-deg.to_radians()).text_align(CanvasTextAlign::Center).text_baseline(CanvasTextBaseline::Middle).fill_text(text, 0.0f32, 0.0f32).restore();
                    continue;
                }
                // the runs to draw, each (text, left edge, width, baseline)
                let mut runs: Vec<(String, f32, f32, f32)> = Vec::new();
                if let Some(layout) = number_format {
                    xlsx_render::paint_number_format(text, layout, *align, |s| measure(&font, s), |s, at| runs.push((s.to_owned(), at, measure(&font, s), *y)));
                } else if let Some(wrap) = wrap {
                    let lines = xlsx_render::wrap::wrap_lines(text, wrap.width, |s| measure(&font, s));
                    for (n, l) in lines.iter().enumerate() {
                        let w = measure(&font, l);
                        runs.push((l.to_string(), start(*x, w, *align), w, xlsx_render::wrap::wrapped_line_y(*y, n, lines.len(), wrap)));
                    }
                } else {
                    let w = measure(&font, text);
                    runs.push((text.to_string(), start(*x, w, *align), w, *y));
                }
                // text inside its cell needs no clip
                let fits = highlight.is_none()
                    && runs.iter().all(|(_, x0, w, by)| *x0 >= clip.x && x0 + w <= clip.x + clip.w && by - px_size * 0.8 >= clip.y && by + px_size * 0.25 <= clip.y + clip.h);
                p = pen.align(p, CanvasTextAlign::Left);
                if !fits {
                    p = p.save().begin_path().rect(bounds(clip)).clip();
                    if let Some(h) = highlight {
                        p = p.fill_color(hex(h)).fill_rect(clip.x, clip.y, clip.w, clip.h).fill_color(c);
                    }
                }
                for (s, x0, w, by) in &runs {
                    p = p.fill_text(s, *x0, *by);
                    p = deco.paint(p, *x0, *by, *w);
                }
                if !fits {
                    p = p.restore();
                }
            }
        }
    }
    pen.clip(p, None)
}

/// A drawing of `list` (for tests and callers without a painter of their own).
pub fn drawing(list: &DisplayList, palette: &Palette, measure: Measure) -> Drawing {
    paint(Painter::new(), list, palette, measure).finish()
}

fn intersect(a: &Rect, b: &Rect) -> Rect {
    let (x0, y0) = (a.x.max(b.x), a.y.max(b.y));
    let (x1, y1) = ((a.x + a.w).min(b.x + b.w), (a.y + a.h).min(b.y + b.h));
    Rect { x: x0, y: y0, w: (x1 - x0).max(0.0), h: (y1 - y0).max(0.0) }
}

/// Where a run `w` wide anchored at `x` starts.
fn start(x: f32, w: f32, a: Align) -> f32 {
    match a {
        Align::Left => x,
        Align::Center => x - w / 2.0,
        Align::Right => x - w,
    }
}

struct TextDeco {
    underline: bool,
    strike: bool,
    /// The font size in pixels.
    size: f32,
    color: Hsla,
}

impl TextDeco {
    fn paint(&self, mut p: Painter, x: f32, baseline: f32, w: f32) -> Painter {
        let t = (self.size / 14.0).max(1.0);
        if self.underline {
            p = p.fill_color(self.color).fill_rect(x, baseline + self.size * 0.1, w, t);
        }
        if self.strike {
            p = p.fill_color(self.color).fill_rect(x, baseline - self.size * 0.3, w, t);
        }
        p
    }
}

/// A border or gridline: solid, dashed or double, on whole pixels as
/// BetterOffice snaps them.
fn line(mut p: Painter, (x1, y1, x2, y2): (f32, f32, f32, f32), width: f32, c: Hsla, style: Option<&str>, rail: Option<f32>) -> Painter {
    let seg = |p: Painter, (x1, y1, x2, y2): (f32, f32, f32, f32), w: f32| p.begin_path().move_to(x1, y1).line_to(x2, y2).line_width(w).stroke();
    p = p.stroke_color(c);
    let horizontal = (y1 - y2).abs() <= (x1 - x2).abs();
    match style {
        Some("double") => {
            let (off, w) = match rail {
                Some(rail) => ((width - rail) / 2.0, rail),
                None => ((width * 0.8).max(0.8), (width * 0.6).max(0.5)),
            };
            let (dx, dy) = if horizontal { (0.0, off) } else { (off, 0.0) };
            p = p.line_dash(&[]);
            p = seg(p, (x1 - dx, y1 - dy, x2 - dx, y2 - dy), w);
            seg(p, (x1 + dx, y1 + dy, x2 + dx, y2 + dy), w)
        }
        Some(s) if !xlsx_render::border_dash(s).is_empty() => {
            let pen = width.round().max(1.0);
            let scale = if s.starts_with("medium") { width / 2.0 } else { width };
            let dash: Vec<Points> = xlsx_render::border_dash(s).iter().map(|d| px(d * scale)).collect();
            p = p.line_dash(&dash);
            p = seg(p, snap((x1, y1, x2, y2), pen), pen);
            p.line_dash(&[])
        }
        _ => {
            p = p.line_dash(&[]);
            seg(p, (x1, y1, x2, y2), width)
        }
    }
}

/// An axis-aligned line centered on device pixels for a `pen`-wide stroke.
fn snap((x1, y1, x2, y2): (f32, f32, f32, f32), pen: f32) -> (f32, f32, f32, f32) {
    let offset = (pen as u32 % 2) as f32 / 2.0;
    if y1 == y2 {
        let y = y1.floor() + offset;
        (x1.round(), y, x2.round(), y)
    } else if x1 == x2 {
        let x = x1.floor() + offset;
        (x, y1.round(), x, y2.round())
    } else {
        (x1, y1, x2, y2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use betteroffice_xlsx::GridMeta;
    use nsh_core::canvas::CanvasOp;

    fn text_cmd(text: &str, color: &str, align: Align) -> DrawCmd {
        text_at(text, color, align, 0.0)
    }

    fn text_at(text: &str, color: &str, align: Align, left: f32) -> DrawCmd {
        DrawCmd::Text {
            x: 10.0,
            y: 15.0,
            text: text.into(),
            font_size: 11.0,
            color: color.into(),
            clip: Rect { x: left, y: 0.0, w: 64.0, h: 20.0 },
            align,
            bold: true,
            italic: false,
            underline: true,
            strike: false,
            highlight: None,
            dashed_underline: false,
            font_family: Some("Calibri".into()),
            number_format: None,
            ghost: false,
            chart: false,
            rotation_deg: None,
            rotation_box: None,
            rotation_valign: None,
            stacked: false,
            wrap: None,
        }
    }

    fn list(commands: Vec<DrawCmd>) -> DisplayList {
        DisplayList { width: 200.0, height: 100.0, commands, grid: GridMeta::default(), hyperlinks: Vec::new(), charts: Vec::new(), truncated: false }
    }

    fn measure(_: &str, s: &str) -> f32 {
        s.chars().count() as f32 * 6.0
    }

    fn texts(d: &Drawing) -> Vec<(String, f32, f32)> {
        d.ops
            .iter()
            .filter_map(|op| match op {
                CanvasOp::FillText { text, x, y, .. } => Some((text.to_string(), x.get(|_| 0.0), y.get(|_| 0.0))),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn maps_fills_lines_and_text_with_their_clips() {
        let l = list(vec![
            DrawCmd::FillRect { x: 0.0, y: 0.0, w: 200.0, h: 100.0, color: SHEET_BG.into(), clip: None },
            DrawCmd::Line { x1: 0.0, y1: 20.5, x2: 200.0, y2: 20.5, width: 1.0, color: GRIDLINE.into(), style: None, rail: None, clip: None },
            text_cmd("Total", AUTO_TEXT, Align::Left),
            text_cmd("a text longer than its cell", AUTO_TEXT, Align::Left),
        ]);
        let d = drawing(&l, &Palette::light(), &measure);
        let fills = d.ops.iter().filter(|op| matches!(op, CanvasOp::FillRect(_))).count();
        // the background, and the two underlines
        assert_eq!(fills, 3);
        assert_eq!(d.ops.iter().filter(|op| matches!(op, CanvasOp::Stroke(_))).count(), 1);
        assert_eq!(texts(&d), vec![("Total".to_owned(), 10.0, 15.0), ("a text longer than its cell".to_owned(), 10.0, 15.0)]);
        // only the text that overflows is clipped, pushed and popped
        let saves = d.ops.iter().filter(|op| matches!(op, CanvasOp::Save)).count();
        let restores = d.ops.iter().filter(|op| matches!(op, CanvasOp::Restore)).count();
        assert_eq!((saves, restores), (1, 1));
        assert_eq!(d.ops.iter().filter(|op| matches!(op, CanvasOp::Clip(..))).count(), 1);
        // the font, color and alignment are set once for both
        assert_eq!(d.ops.iter().filter(|op| matches!(op, CanvasOp::Font(_))).count(), 1);
        assert_eq!(d.ops.iter().filter(|op| matches!(op, CanvasOp::TextAlign(_))).count(), 1);
        // Calibri drawn as its bundled stand-in
        assert!(d.ops.iter().any(|op| matches!(op, CanvasOp::Font(f) if f.weight == 700 && f.family.as_deref() == Some("Carlito"))));
    }

    #[test]
    fn right_aligned_text_ends_at_its_anchor() {
        let d = drawing(&list(vec![text_cmd("42", AUTO_TEXT, Align::Right)]), &Palette::light(), &measure);
        // runs are placed by their left edge: a run's width left of the anchor
        assert_eq!(texts(&d), vec![("42".to_owned(), 10.0 - 12.0, 15.0)]);
        let underline = d.ops.iter().find_map(|op| match op {
            CanvasOp::FillRect([x, ..]) => Some(x.get(|_| 0.0)),
            _ => None,
        });
        assert_eq!(underline, Some(10.0 - 12.0));
    }

    #[test]
    fn dark_mode_recolors_the_defaults_but_not_the_authors() {
        let l = list(vec![
            DrawCmd::FillRect { x: 0.0, y: 0.0, w: 200.0, h: 100.0, color: SHEET_BG.into(), clip: None },
            // an author's yellow fill under the first text
            DrawCmd::FillRect { x: 0.0, y: 0.0, w: 64.0, h: 20.0, color: "#ffff00".into(), clip: None },
            text_cmd("on yellow", AUTO_TEXT, Align::Left),
            text_at("plain", AUTO_TEXT, Align::Left, 64.0),
            text_at("red", "#ff0000", Align::Left, 128.0),
        ]);
        let dark = Palette::dark();
        let d = drawing(&l, &dark, &measure);
        let mut fill = None;
        let mut seen = Vec::new();
        for op in &d.ops {
            match op {
                CanvasOp::FillStyle(nsh_core::canvas::Paint::Color(c)) => fill = Some(Hsla::from(*c)),
                CanvasOp::FillText { text, .. } => seen.push((text.to_string(), fill)),
                CanvasOp::FillRect(_) if seen.is_empty() && fill.is_some() => {}
                _ => {}
            }
        }
        let close = |a: Option<Hsla>, b: Hsla| a.is_some_and(|a| (a.l - b.l).abs() < 0.02 && (a.h - b.h).abs() < 0.02);
        assert!(close(seen[0].1, hex(AUTO_TEXT)), "text on an author's fill stays black");
        assert!(close(seen[1].1, dark.text), "automatic text turns light");
        assert!(close(seen[2].1, hex("#ff0000")), "an author's color stays");
        // the sheet's background is the dark one
        let first_fill = d.ops.iter().find_map(|op| match op {
            CanvasOp::FillStyle(nsh_core::canvas::Paint::Color(c)) => Some(Hsla::from(*c)),
            _ => None,
        });
        assert!(close(first_fill, dark.bg));
    }

    #[test]
    fn dashed_and_double_borders() {
        let l = list(vec![
            DrawCmd::Line { x1: 0.0, y1: 10.0, x2: 50.0, y2: 10.0, width: 1.0, color: "#000000".into(), style: Some("dashed".into()), rail: None, clip: None },
            DrawCmd::Line { x1: 0.0, y1: 30.0, x2: 50.0, y2: 30.0, width: 3.0, color: "#000000".into(), style: Some("double".into()), rail: Some(1.0), clip: None },
        ]);
        let d = drawing(&l, &Palette::light(), &measure);
        assert!(d.ops.iter().any(|op| matches!(op, CanvasOp::LineDash(d) if d.len() == 2)));
        // one stroke for the dashed line, two rails for the double
        assert_eq!(d.ops.iter().filter(|op| matches!(op, CanvasOp::Stroke(_))).count(), 3);
    }

    #[test]
    fn wrapped_text_breaks_into_lines() {
        let mut cmd = text_cmd("alpha beta gamma", AUTO_TEXT, Align::Left);
        if let DrawCmd::Text { wrap, underline, .. } = &mut cmd {
            *wrap = Some(xlsx_render::TextWrap { width: 40.0, line_height: 15.0, valign: xlsx_render::TextValign::Top });
            *underline = false;
        }
        let d = drawing(&list(vec![cmd]), &Palette::light(), &measure);
        let t = texts(&d);
        assert_eq!(t.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), vec!["alpha", "beta", "gamma"]);
        assert!(t[1].2 > t[0].2 && t[2].2 > t[1].2);
    }

    #[test]
    fn a_real_list_converts() {
        let mut sheet = betteroffice_xlsx::Sheet::new("S");
        sheet.set_cell(betteroffice_xlsx::CellRef::new(0, 0), betteroffice_xlsx::Cell { value: betteroffice_xlsx::CellValue::Text { value: "hello".into() }, ..Default::default() });
        sheet.set_cell(betteroffice_xlsx::CellRef::new(1, 1), betteroffice_xlsx::Cell { value: betteroffice_xlsx::CellValue::Number { value: 3.5 }, ..Default::default() });
        let model = betteroffice_xlsx::WorkbookModel { sheets: vec![sheet], ..Default::default() };
        let wb = betteroffice_xlsx::Workbook::from_model(model).unwrap();
        let dl = wb.display_list(&betteroffice_xlsx::Viewport { x: 0.0, y: 0.0, width: 300.0, height: 100.0 }).unwrap();
        let d = drawing(&dl, &Palette::light(), &measure);
        let t: Vec<String> = texts(&d).into_iter().map(|t| t.0).collect();
        assert_eq!(t, vec!["hello".to_owned(), "3.5".into()]);
    }
}
