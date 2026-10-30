//! The sheet's grid, in layers as nsh's `DataTable` does it: the cells
//! (canvases of the display list, which take no pointer), a hit layer over
//! them (a vertical scroll view of a horizontal one of an empty box the
//! sheet's size: the platform scrolls it with inertia and scroll bars, and
//! its box takes presses, drags and the context menu), the selection, the
//! headers and the cell editor.
//!
//! A display list covers the window in view and a margin around it: a
//! scroll moves the canvases inside their panes (an offset), and only a
//! scroll past the margin, a new version, a resize, the zoom or the theme
//! draws again. Frozen panes are the same drawing shown four times, each
//! pane clipped and moved along its own axes (the list draws the frozen
//! rows and columns at its top left, the body after them).

use std::sync::Arc;

use betteroffice_xlsx::{GridGeometry, Viewport};
use nsh_core::canvas::{CanvasTextAlign, CanvasTextBaseline, Drawing};
use nsh_core::prelude::*;

use super::select::{self, LAST_COL, LAST_ROW};
use super::{Shape, SheetUi, edit};
use crate::office::draw::{self, Palette};
use crate::theme;

/// The row headers' width and the column headers' height, in points.
pub(crate) const ROW_HEAD: f32 = 46.0;
pub(crate) const COL_HEAD: f32 = 22.0;
/// The sheet pixels drawn past each edge of the view.
const MARGIN: f32 = 320.0;
/// Platforms keep scroll offsets in 32-bit floats: past this the content
/// stops growing (a million rows of 20 px would be 20M points).
const MAX_EXTENT: f32 = 3_000_000.0;

/// Where the panes are: the zoom, the frozen panes' size on screen, and
/// the body's scroll in sheet pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Frame {
    pub zoom: f32,
    /// The frozen columns' width and rows' height, in sheet pixels.
    pub fw: f32,
    pub fh: f32,
    pub scroll: (f32, f32),
}

impl Frame {
    pub(crate) fn of(g: &GridGeometry, frozen: (u32, u32), scroll: (f32, f32), zoom: f32) -> Frame {
        Frame { zoom, fw: g.col_x(frozen.1), fh: g.row_y(frozen.0), scroll }
    }

    /// A point of the cells area (points) in sheet pixels.
    pub(crate) fn to_sheet(&self, x: f32, y: f32) -> (f32, f32) {
        let (sx, sy) = (x / self.zoom, y / self.zoom);
        (if sx < self.fw { sx } else { sx + self.scroll.0 }, if sy < self.fh { sy } else { sy + self.scroll.1 })
    }

    /// A sheet x on screen (frozen columns stay put; the body scrolls).
    pub(crate) fn screen_x(&self, x: f32) -> f32 {
        (if x < self.fw { x } else { x - self.scroll.0 }) * self.zoom
    }

    pub(crate) fn screen_y(&self, y: f32) -> f32 {
        (if y < self.fh { y } else { y - self.scroll.1 }) * self.zoom
    }

    /// The body window in view for a cells area `w`×`h` points: its
    /// sheet pixels, from the first unfrozen cell's edge.
    pub(crate) fn body(&self, w: f32, h: f32) -> (f32, f32, f32, f32) {
        let x0 = self.fw + self.scroll.0;
        let y0 = self.fh + self.scroll.1;
        (x0, y0, (w / self.zoom - self.fw).max(0.0), (h / self.zoom - self.fh).max(0.0))
    }
}

/// The window to draw for an area `w`×`h`: the view and [`MARGIN`]
/// around it (the scroll offsets of BetterOffice's viewport count from
/// the first unfrozen cell).
pub(crate) fn window(f: &Frame, w: f32, h: f32) -> Viewport {
    let x = (f.scroll.0 - MARGIN).max(0.0);
    let y = (f.scroll.1 - MARGIN).max(0.0);
    Viewport { x, y, width: w / f.zoom + MARGIN * 2.0, height: h / f.zoom + MARGIN * 2.0 }
}

/// Whether `vp` (drawn) still covers the view.
pub(crate) fn covers(vp: &Viewport, f: &Frame, w: f32, h: f32) -> bool {
    let (bw, bh) = ((w / f.zoom - f.fw).max(0.0), (h / f.zoom - f.fh).max(0.0));
    let (body_w, body_h) = (vp.width - f.fw, vp.height - f.fh);
    f.scroll.0 >= vp.x && f.scroll.1 >= vp.y && f.scroll.0 + bw <= vp.x + body_w && f.scroll.1 + bh <= vp.y + body_h
}

/// The cell under a point of the cells area.
pub(crate) fn cell_at(g: &GridGeometry, f: &Frame, x: f32, y: f32) -> (u32, u32) {
    let (sx, sy) = f.to_sheet(x, y);
    (g.row_at_y(sy).min(LAST_ROW), g.col_at_x(sx).min(LAST_COL))
}

/// A cell's rect on screen (points in the cells area).
pub(crate) fn cell_rect(g: &GridGeometry, f: &Frame, (r, c): (u32, u32)) -> (f32, f32, f32, f32) {
    let (x0, x1) = (f.screen_x(g.col_x(c)), f.screen_x(g.col_x(c + 1)));
    let (y0, y1) = (f.screen_y(g.row_y(r)), f.screen_y(g.row_y(r + 1)));
    (x0, y0, x1 - x0, y1 - y0)
}

/// The scroll that brings `cell` into a `w`×`h` view (unchanged when it
/// shows, or sits in a frozen pane).
pub(crate) fn reveal(g: &GridGeometry, f: &Frame, (r, c): (u32, u32), w: f32, h: f32) -> (f32, f32) {
    let (bx, by, bw, bh) = f.body(w, h);
    let mut s = f.scroll;
    let (x0, x1) = (g.col_x(c), g.col_x(c + 1));
    if x0 >= f.fw {
        if x0 < bx {
            s.0 = x0 - f.fw;
        } else if x1 > bx + bw {
            s.0 = (x1 - f.fw - bw).min(x0 - f.fw).max(0.0);
        }
    }
    let (y0, y1) = (g.row_y(r), g.row_y(r + 1));
    if y0 >= f.fh {
        if y0 < by {
            s.1 = y0 - f.fh;
        } else if y1 > by + bh {
            s.1 = (y1 - f.fh - bh).min(y0 - f.fh).max(0.0);
        }
    }
    s
}

/// The body's content size in points: the used range and room past it.
fn content(shape: &Shape, f: &Frame, w: f32, h: f32) -> (f32, f32) {
    let g = &shape.0.geometry;
    let (rows, cols) = shape.0.used;
    let right = g.col_x((cols + 26).min(LAST_COL + 1)).max(g.col_x(cols) + w / f.zoom);
    let bottom = g.row_y((rows + 200).min(LAST_ROW + 1)).max(g.row_y(rows) + h / f.zoom);
    // the content keeps up with a scroll that went past it
    let right = right.max(f.fw + f.scroll.0 + w / f.zoom * 2.0);
    let bottom = bottom.max(f.fh + f.scroll.1 + h / f.zoom * 2.0);
    (((right - f.fw) * f.zoom + f.fw * f.zoom).min(MAX_EXTENT), ((bottom - f.fh) * f.zoom + f.fh * f.zoom).min(MAX_EXTENT))
}

/// The frame now (tracked).
pub(crate) fn frame(u: &SheetUi) -> Option<(Shape, Frame)> {
    let shape = u.shape.get()?;
    let f = Frame::of(&shape.0.geometry, shape.0.frozen, u.scroll.get(), u.zoom.get());
    Some((shape, f))
}

/// Asks the worker for the window in view unless the list in hand (or
/// the one asked) covers it.
fn want(u: &SheetUi) {
    let Some((shape, f)) = untracked(|| frame(u)) else { return };
    let (w, h) = u.area.get_untracked();
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let (rev, sheet) = (shape.0.rev, shape.0.active);
    let fresh = |r: u64, s: usize, vp: &Viewport, z: f32| r == rev && s == sheet && z == f.zoom && covers(vp, &f, w, h);
    if u.shown.with_untracked(|s| s.as_ref().is_some_and(|s| fresh(s.rev, s.sheet, &s.vp, s.scale))) {
        return;
    }
    if u.asked.get().is_some_and(|(r, s, vp, z)| fresh(r, s, &vp, z)) {
        return;
    }
    let vp = window(&f, w, h);
    u.asked.set(Some((rev, sheet, vp, f.zoom)));
    crate::office::with_session(|s| s.send(super::doc::Cmd::View { sheet, vp, scale: f.zoom }));
}

thread_local! {
    /// The scroll views (keyboard moves scroll them to the active cell).
    static SCROLLERS: std::cell::Cell<Option<(ElementRef, ElementRef)>> = const { std::cell::Cell::new(None) };
}

/// Scrolls the view so the active cell shows.
pub(crate) fn reveal_active(u: &SheetUi) {
    let Some((shape, f)) = untracked(|| frame(u)) else { return };
    let (w, h) = u.area.get_untracked();
    let s = reveal(&shape.0.geometry, &f, u.sel.get_untracked().focus, w, h);
    if s != f.scroll {
        u.scroll.set(s);
        if let Some((v, hz)) = SCROLLERS.with(std::cell::Cell::get) {
            v.scroll_to(point(px(0.0), px(s.1 * f.zoom)), false);
            hz.scroll_to(point(px(s.0 * f.zoom), px(0.0)), false);
        }
    }
}

/// The cells: one drawing, shown in up to four panes.
fn cells(u: &'static SheetUi) -> impl IntoElement {
    let measure = nsh_core::element::text_measurer();
    // the drawing of the list in hand, recorded once per list and theme
    let drawing = signal_with(Arc::new(Drawing::default()), SignalEquals::Never);
    effect(move || {
        let palette = if dark() { Palette::dark() } else { Palette::light() };
        let d = u.shown.with(|s| s.as_ref().map(|s| draw::drawing(&s.list, &palette, &|font, t| measure(font, t).0)));
        drawing.set(Arc::new(d.unwrap_or_default()));
    });
    // where the list's origin sits relative to the body pane: the
    // scroll's distance into the drawn window
    let shift = move || -> (f32, f32) {
        let Some(s) = u.shown.get() else { return (0.0, 0.0) };
        let (sx, sy) = u.scroll.get();
        ((sx - s.vp.x) * s.scale, (sy - s.vp.y) * s.scale)
    };
    let frozen = move || -> (f32, f32) {
        u.shape.with(|s| s.as_ref().map_or((0.0, 0.0), |s| (s.0.geometry.col_x(s.0.frozen.1), s.0.geometry.row_y(s.0.frozen.0)))).to_owned()
    };
    let z = move || u.zoom.get();
    // a pane at (x, y) of size (w, h) in the cells area, showing the
    // drawing moved by (dx, dy)
    let pane = move |place: fn((f32, f32), f32) -> (f32, f32), moves: (bool, bool)| {
        let d = drawing;
        view()
            .absolute()
            .overflow_hidden()
            .pointer_events(PointerEvents::None)
            .left(move || px(place(frozen(), z()).0))
            .top(move || px(place(frozen(), z()).1))
            .right(px(0.0))
            .bottom(px(0.0))
            .child(
                canvas(move || Painter::from((*d.get()).clone()))
                    .absolute()
                    .left(move || px(-place(frozen(), z()).0 - if moves.0 { shift().0 } else { 0.0 }))
                    .top(move || px(-place(frozen(), z()).1 - if moves.1 { shift().1 } else { 0.0 }))
                    .w(move || px(u.shown.with(|s| s.as_ref().map_or(0.0, |s| s.list.width))))
                    .h(move || px(u.shown.with(|s| s.as_ref().map_or(0.0, |s| s.list.height)))),
            )
    };
    let has_cols = move || frozen().0 > 0.0;
    let has_rows = move || frozen().1 > 0.0;
    view()
        .absolute()
        .size_full()
        .overflow_hidden()
        .pointer_events(PointerEvents::None)
        // the body: both ways
        .child(pane(|(fw, fh), z| (fw * z, fh * z), (true, true)))
        // the frozen rows: across only
        .child(when(has_rows, move || pane(|(fw, _), z| (fw * z, 0.0), (true, false)).bottom(Length::Auto).h(move || px(frozen().1 * z())).into_element()))
        // the frozen columns: down only
        .child(when(has_cols, move || pane(|(_, fh), z| (0.0, fh * z), (false, true)).right(Length::Auto).w(move || px(frozen().0 * z())).into_element()))
        .child(when(move || has_rows() && has_cols(), move || pane(|_, _| (0.0, 0.0), (false, false)).right(Length::Auto).bottom(Length::Auto).w(move || px(frozen().0 * z())).h(move || px(frozen().1 * z())).into_element()))
}

/// The selection over the cells: the range tinted, the active cell and
/// the range outlined.
fn selection(u: &'static SheetUi) -> impl IntoElement {
    canvas(move || {
        let p = Painter::new();
        let Some((shape, f)) = frame(u) else { return p };
        let g = &shape.0.geometry;
        let sel = u.sel.get();
        let (w, h) = u.area.get();
        let accent = color(theme::ACCENT);
        let ((r0, c0), (r1, c1)) = sel.range();
        let clamp_x = |x: f32| x.clamp(-4.0, w + 4.0);
        let clamp_y = |y: f32| y.clamp(-4.0, h + 4.0);
        let (x0, y0) = (clamp_x(f.screen_x(g.col_x(c0))), clamp_y(f.screen_y(g.row_y(r0))));
        let (x1, y1) = (clamp_x(f.screen_x(g.col_x(c1.saturating_add(1).min(LAST_COL + 1)))), clamp_y(f.screen_y(g.row_y(r1.saturating_add(1).min(LAST_ROW + 1)))));
        let mut p = p;
        if !sel.is_single() {
            let mut tint = accent;
            tint.a = 0.12;
            p = p.fill_color(tint).fill_rect(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0));
        }
        let (ax, ay, aw, ah) = cell_rect(g, &f, sel.active);
        // the active cell shows through the tint
        if !sel.is_single() {
            p = p.clear_rect(Bounds { origin: point(px(ax), px(ay)), size: size(px(aw.max(0.0)), px(ah.max(0.0))) });
        }
        p = p.stroke_color(accent).line_width(2.0f32).line_dash(&[]);
        p = p.begin_path().rect(Bounds { origin: point(px(x0), px(y0)), size: size(px((x1 - x0).max(0.0)), px((y1 - y0).max(0.0))) }).stroke();
        // the fill handle's corner
        p.fill_color(accent).fill_rect(x1 - 3.0, y1 - 3.0, 6.0f32, 6.0f32)
    })
    .absolute()
    .size_full()
    .pointer_events(PointerEvents::None)
}

/// The column letters (`cols`) or row numbers along the cells, the
/// selected ones marked.
fn headers(u: &'static SheetUi, cols: bool) -> CanvasView {
    canvas(move || {
        let mut p = Painter::new();
        let Some((shape, f)) = frame(u) else { return p };
        let g = &shape.0.geometry;
        let (w, h) = u.area.get();
        let sel = u.sel.get();
        let ((r0, c0), (r1, c1)) = sel.range();
        let (text_c, line_c, on_c) = (color(theme::MUTED), color(theme::BORDER_S), color(theme::ACCENT));
        let mut on_bg = on_c;
        on_bg.a = 0.14;
        let font = format!("{}px system-ui", (11.0 * f.zoom.clamp(0.8, 1.4)).round());
        p = p.font(&font).text_align(CanvasTextAlign::Center).text_baseline(CanvasTextBaseline::Middle);
        let frozen = if cols { shape.0.frozen.1 } else { shape.0.frozen.0 };
        // the frozen ones, then the body's from the scroll on
        let first_body = if cols { g.col_at_x(f.fw + f.scroll.0) } else { g.row_at_y(f.fh + f.scroll.1) };
        let limit = if cols { w } else { h };
        let edge = |i: u32| if cols { (f.screen_x(g.col_x(i)), f.screen_x(g.col_x(i + 1))) } else { (f.screen_y(g.row_y(i)), f.screen_y(g.row_y(i + 1))) };
        let last = if cols { LAST_COL } else { LAST_ROW };
        let mut i = 0;
        let mut n = 0;
        while i <= last && n < 2000 {
            if i == frozen && i < first_body {
                i = first_body;
            }
            let (a, b) = edge(i);
            if a > limit {
                break;
            }
            n += 1;
            if b > a {
                let on = if cols { (c0..=c1).contains(&i) } else { (r0..=r1).contains(&i) };
                let (x, y, ww, hh) = if cols { (a, 0.0, b - a, COL_HEAD) } else { (0.0, a, ROW_HEAD, b - a) };
                if on {
                    p = p.fill_color(on_bg).fill_rect(x, y, ww, hh);
                }
                p = p.fill_color(if on { on_c } else { text_c });
                let label = if cols { select::col_name(i) } else { (i + 1).to_string() };
                p = p.fill_text(&label, x + ww / 2.0, y + hh / 2.0);
                p = p.fill_color(line_c);
                p = if cols { p.fill_rect(b - 0.5, 0.0f32, 0.5f32, COL_HEAD) } else { p.fill_rect(0.0f32, b - 0.5, ROW_HEAD, 0.5f32) };
            }
            i += 1;
        }
        p
    })
}

/// The hit layer: scrolls, presses, drags, double clicks, the menu.
fn hits(u: &'static SheetUi) -> impl IntoElement {
    let v_ref = element_ref();
    let h_ref = element_ref();
    SCROLLERS.with(|s| s.set(Some((v_ref.clone(), h_ref.clone()))));
    let size = move || -> (f32, f32) {
        let Some((shape, f)) = frame(u) else { return (0.0, 0.0) };
        let (w, h) = u.area.get();
        content(&shape, &f, w, h)
    };
    // a content point (the box scrolls) as a point of the cells area
    let area_point = move |x: f32, y: f32| {
        let (sx, sy) = u.scroll.get_untracked();
        let z = u.zoom.get_untracked();
        (x - sx * z, y - sy * z)
    };
    let cell_of = move |x: f32, y: f32| -> Option<(u32, u32)> {
        let (shape, f) = untracked(|| frame(u))?;
        let (ax, ay) = area_point(x, y);
        Some(cell_at(&shape.0.geometry, &f, ax, ay))
    };
    let dragging = std::rc::Rc::new(std::cell::Cell::new(false));
    let (d1, d2, d3) = (dragging.clone(), dragging.clone(), dragging);
    scroll()
        .element_ref(&v_ref)
        .absolute()
        .size_full()
        .on_scroll(move |e: &ScrollEvent| {
            let z = u.zoom.get_untracked();
            u.scroll.update(|s| s.1 = e.offset.y.0 / z);
        })
        .child(
            scroll()
                .horizontal()
                .element_ref(&h_ref)
                .h(move || px(size().1))
                .on_scroll(move |e: &ScrollEvent| {
                    let z = u.zoom.get_untracked();
                    u.scroll.update(|s| s.0 = e.offset.x.0 / z);
                })
                .child(
                    view()
                        .w(move || px(size().0))
                        .h(move || px(size().1))
                        .on_press_down(move |e: &PressEvent| {
                            let Some(cell) = cell_of(e.position.x.0, e.position.y.0) else { return };
                            d1.set(true);
                            edit::press(cell, e.modifiers.shift, e.modifiers.cmd);
                        })
                        .on_press_up(move |_: &PressEvent| d2.set(false))
                        .on_press(move |e: &PressEvent| {
                            if e.clicks == 2
                                && let Some(cell) = cell_of(e.position.x.0, e.position.y.0)
                            {
                                edit::begin(cell, None);
                            }
                        })
                        .on_mouse_move(move |e: &MouseEvent| {
                            if !d3.get() || e.pressed_button.is_none() {
                                d3.set(false);
                                return;
                            }
                            if let Some(cell) = cell_of(e.position.x.0, e.position.y.0) {
                                let s = u.sel.get_untracked();
                                if s.focus != cell {
                                    u.sel.set(s.drag(cell));
                                }
                            }
                        })
                        .on_context_menu(move |e: &ContextMenuEvent| {
                            if let Some(cell) = cell_of(e.position.x.0, e.position.y.0) {
                                edit::menu(cell, e.page_position.x.0, e.page_position.y.0);
                            }
                        }),
                ),
        )
}

/// The headers take presses: a whole column or row.
fn header_hits(u: &'static SheetUi, cols: bool) -> View {
    view().absolute().size_full().on_press_down(move |e: &PressEvent| {
        let Some((shape, f)) = untracked(|| frame(u)) else { return };
        let g = &shape.0.geometry;
        let s = u.sel.get_untracked();
        let next = if cols {
            let (x, _) = f.to_sheet(e.position.x.0, 0.0);
            s.col(g.col_at_x(x).min(LAST_COL), e.modifiers.shift)
        } else {
            let (_, y) = f.to_sheet(0.0, e.position.y.0);
            s.row(g.row_at_y(y).min(LAST_ROW), e.modifiers.shift)
        };
        edit::commit_editing();
        u.sel.set(next);
        edit::read_active();
    })
}

/// The grid: headers, cells, the selection, the editor over the cell.
pub(crate) fn grid(u: &'static SheetUi) -> impl IntoElement {
    // the worker draws what comes into view
    effect(move || {
        u.shape.with(|_| ());
        u.scroll.with(|_| ());
        u.area.with(|_| ());
        u.zoom.with(|_| ());
        u.shown.with(|_| ());
        want(u);
    });
    let corner_bg = theme::SUBTLE;
    view()
        .flex_1()
        .bg(theme::PANE)
        // column letters
        .child(
            view()
                .h(px(COL_HEAD))
                .flex_row()
                .bg(corner_bg)
                .border_b(px(0.5))
                .border_b_color(theme::BORDER_S)
                .child(
                    view()
                        .label("Select all")
                        .w(px(ROW_HEAD))
                        .h_full()
                        .border_r(px(0.5))
                        .border_r_color(theme::BORDER_S)
                        .on_press(move || edit::select_all()),
                )
                .child(view().flex_1().h_full().overflow_hidden().child(headers(u, true).absolute().size_full()))
                .child(header_hits(u, true).left(px(ROW_HEAD))),
        )
        .child(
            view()
                .flex_1()
                .flex_row()
                // row numbers
                .child(
                    view()
                        .w(px(ROW_HEAD))
                        .h_full()
                        .overflow_hidden()
                        .bg(corner_bg)
                        .border_r(px(0.5))
                        .border_r_color(theme::BORDER_S)
                        .child(headers(u, false).absolute().size_full())
                        .child(header_hits(u, false)),
                )
                // the cells area
                .child(
                    view()
                        .flex_1()
                        .h_full()
                        .overflow_hidden()
                        .on_layout(move |b: &Bounds<Points>| {
                            let s = (b.size.width.0, b.size.height.0);
                            if u.area.get_untracked() != s {
                                u.area.set(s);
                            }
                        })
                        .child(cells(u))
                        .child(selection(u))
                        .child(hits(u))
                        .child(edit::cell_editor(u)),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use betteroffice_xlsx::{Cell, CellRef, CellValue, FreezePane, Sheet, Stylesheet};

    fn geometry(frozen: bool) -> GridGeometry {
        let mut s = Sheet::new("S");
        s.set_cell(CellRef::new(0, 0), Cell { value: CellValue::Number { value: 1.0 }, ..Default::default() });
        if frozen {
            s.freeze_pane = Some(FreezePane { rows: 1, cols: 1, top_left: CellRef::new(1, 1) });
        }
        GridGeometry::new(&s, &Stylesheet::default())
    }

    #[test]
    fn points_map_to_cells_across_the_panes() {
        let g = geometry(true);
        let (cw, rh) = (g.col_x(1), g.row_y(1));
        // scrolled 10 columns and 100 rows into the body
        let f = Frame::of(&g, (1, 1), (cw * 10.0, rh * 100.0), 1.0);
        // the frozen corner stays A1
        assert_eq!(cell_at(&g, &f, 2.0, 2.0), (0, 0));
        // just past the frozen column, the body's first column in view
        assert_eq!(cell_at(&g, &f, cw + 1.0, rh + 1.0), (101, 11));
        // the frozen row over a scrolled column
        assert_eq!(cell_at(&g, &f, cw + 1.0, 2.0), (0, 11));
        // and back: the cell's rect starts where the point fell
        let (x, y, w, h) = cell_rect(&g, &f, (101, 11));
        assert_eq!((x, y), (cw, rh));
        assert_eq!((w, h), (cw, rh));
        // at 2x everything doubles
        let f2 = Frame { zoom: 2.0, ..f };
        assert_eq!(cell_at(&g, &f2, (cw + 1.0) * 2.0, (rh + 1.0) * 2.0), (101, 11));
    }

    #[test]
    fn the_window_covers_the_view_until_a_scroll_leaves_its_margin() {
        let g = geometry(false);
        let f = Frame::of(&g, (0, 0), (0.0, 0.0), 1.0);
        let vp = window(&f, 800.0, 600.0);
        assert_eq!((vp.x, vp.y), (0.0, 0.0));
        assert!(covers(&vp, &f, 800.0, 600.0));
        let near = Frame { scroll: (MARGIN, MARGIN), ..f };
        assert!(covers(&vp, &near, 800.0, 600.0));
        let far = Frame { scroll: (0.0, MARGIN * 2.0 + 1.0), ..f };
        assert!(!covers(&vp, &far, 800.0, 600.0));
        let again = window(&far, 800.0, 600.0);
        assert!(covers(&again, &far, 800.0, 600.0));
        assert_eq!(again.y, MARGIN + 1.0);
        // a bigger view needs a new window
        assert!(!covers(&vp, &f, 800.0, 600.0 + MARGIN * 3.0));
    }

    #[test]
    fn reveal_scrolls_just_enough() {
        let g = geometry(true);
        let rh = g.row_y(1);
        let f = Frame::of(&g, (1, 1), (0.0, 0.0), 1.0);
        let h = rh * 11.0; // the frozen row and ten body rows
        assert_eq!(reveal(&g, &f, (5, 1), 800.0, h), (0.0, 0.0));
        // row 12 sits just below: scroll it to the bottom edge
        let s = reveal(&g, &f, (12, 1), 800.0, h);
        assert_eq!(s.1, rh * 2.0);
        // back up above the view: it goes to the top
        let f = Frame { scroll: (0.0, rh * 50.0), ..f };
        assert_eq!(reveal(&g, &f, (3, 1), 800.0, h).1, rh * 2.0);
        // a frozen cell needs no scroll
        assert_eq!(reveal(&g, &f, (0, 0), 800.0, h), f.scroll);
    }
}
