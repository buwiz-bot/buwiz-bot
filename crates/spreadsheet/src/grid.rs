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
use super::{Shape, SheetUi, chart, edit, resize};
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
                    // A frozen pane shows only the frozen rows or columns
                    // at the drawing's top left: its canvas is that strip,
                    // not the whole window, which Core Animation would draw
                    // in full for each pane.
                    .w(move || px(u.shown.with(|s| s.as_ref().map_or(0.0, |s| if moves.0 { s.list.width } else { (frozen().0 * s.scale).min(s.list.width) }))))
                    .h(move || px(u.shown.with(|s| s.as_ref().map_or(0.0, |s| if moves.1 { s.list.height } else { (frozen().1 * s.scale).min(s.list.height) })))),
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

/// Where the selection's parts go in the cells area: the range (clamped
/// a little past the area) and the active cell inside it.
#[derive(Clone, Copy, PartialEq, Default)]
struct SelRects {
    range: (f32, f32, f32, f32),
    active: (f32, f32, f32, f32),
    single: bool,
}

/// The selection's drawn range in the cells area: (x0, y0, x1, y1).
#[cfg(test)]
pub(crate) fn sel_range(u: &SheetUi) -> Option<(f32, f32, f32, f32)> {
    sel_rects(u).map(|r| r.range)
}

fn sel_rects(u: &SheetUi) -> Option<SelRects> {
    let (shape, f) = frame(u)?;
    let g = &shape.0.geometry;
    let sel = u.sel.get();
    let (w, h) = u.area.get();
    // drawn over the merged cells it touches, as the web draws it
    let merges = u.shown.with(|s| s.as_ref().map(|s| s.merges.clone())).unwrap_or_default();
    let ((r0, c0), (r1, c1)) = select::expand(sel.range(), &merges);
    let ((ar0, ac0), (ar1, ac1)) = select::expand((sel.active, sel.active), &merges);
    let clamp_x = |x: f32| x.clamp(-4.0, w + 4.0);
    let clamp_y = |y: f32| y.clamp(-4.0, h + 4.0);
    let (x0, y0) = (clamp_x(f.screen_x(g.col_x(c0))), clamp_y(f.screen_y(g.row_y(r0))));
    let (x1, y1) = (clamp_x(f.screen_x(g.col_x(c1.saturating_add(1).min(LAST_COL + 1)))), clamp_y(f.screen_y(g.row_y(r1.saturating_add(1).min(LAST_ROW + 1)))));
    let (ax, ay, _, _) = cell_rect(g, &f, (ar0, ac0));
    let (bx, by, bw, bh) = cell_rect(g, &f, (ar1, ac1));
    let single = ((r0, c0), (r1, c1)) == ((ar0, ac0), (ar1, ac1));
    Some(SelRects { range: (x0, y0, x1, y1), active: (ax, ay, (bx + bw).max(ax), (by + bh).max(ay)), single })
}

/// The range's tint around the active cell, which shows through it: the
/// bands above, below, left and right of it (empty for a single cell).
fn tint_bands(s: &SelRects) -> [(f32, f32, f32, f32); 4] {
    if s.single {
        return [(0.0, 0.0, 0.0, 0.0); 4];
    }
    let (x0, y0, x1, y1) = s.range;
    let ax0 = s.active.0.clamp(x0, x1);
    let ax1 = s.active.2.clamp(x0, x1);
    let ay0 = s.active.1.clamp(y0, y1);
    let ay1 = s.active.3.clamp(y0, y1);
    [
        (x0, y0, x1 - x0, ay0 - y0),
        (x0, ay1, x1 - x0, y1 - ay1),
        (x0, ay0, ax0 - x0, ay1 - ay0),
        (ax1, ay0, x1 - ax1, ay1 - ay0),
    ]
}

/// The selection over the cells: the range tinted, the active cell and
/// the range outlined (hidden while a chart is selected). Views that move: a drag or a scroll changes their
/// frames, where a canvas the area's size was recorded and drawn again
/// (and its whole bitmap sent to the GPU) on every step.
fn selection(u: &'static SheetUi) -> impl IntoElement {
    let rects = computed(move || sel_rects(u).unwrap_or_default());
    let shown = move || rects.with(|r| r.range != (0.0, 0.0, 0.0, 0.0)) && u.chart.with(Option::is_none);
    // a box at `at()` (x, y, w, h)
    let place = move |v: View, at: std::rc::Rc<dyn Fn() -> (f32, f32, f32, f32)>| {
        let (a, b, c, d) = (at.clone(), at.clone(), at.clone(), at);
        v.absolute()
            .pointer_events(PointerEvents::None)
            .left(move || px(a().0))
            .top(move || px(b().1))
            .w(move || px(c().2.max(0.0)))
            .h(move || px(d().3.max(0.0)))
    };
    let tint = move || {
        let mut t = color(theme::ACCENT);
        t.a = 0.12;
        t
    };
    let mut layer = view().absolute().size_full().pointer_events(PointerEvents::None).opacity(move || if shown() { 1.0f32 } else { 0.0 });
    for k in 0..4 {
        layer = layer.child(place(view(), std::rc::Rc::new(move || rects.with(|r| tint_bands(r)[k]))).bg(tint));
    }
    // the outline: a 2pt stroke centered on the range's edges (no fill
    // handle: nothing drags one, and the web draws none)
    layer.child(
        place(
            view(),
            std::rc::Rc::new(move || rects.with(|r| {
                let (x0, y0, x1, y1) = r.range;
                (x0 - 1.0, y0 - 1.0, (x1 - x0).max(0.0) + 2.0, (y1 - y0).max(0.0) + 2.0)
            })),
        )
        .border(px(2.0))
        .border_color(theme::ACCENT),
    )
}

/// The web editor's chart selection: `2px solid #217346` inside the frame,
/// shadowed `0 1px 6px rgba(0,0,0,0.25)`, filled `rgba(33,115,70,0.08)` while
/// dragged.
fn chart_outline(u: &'static SheetUi) -> impl IntoElement {
    let rect = computed(move || chart::outline(u));
    let at = move |k: usize| move || px(rect.with(|r| r.map_or(0.0, |r| [r.0, r.1, r.2.max(0.0), r.3.max(0.0)][k])));
    let green = draw::hex("#217346");
    let mut tint = green;
    tint.a = 0.08;
    let mut clear = green;
    clear.a = 0.0;
    view()
        .absolute()
        .pointer_events(PointerEvents::None)
        .left(at(0))
        .top(at(1))
        .w(at(2))
        .h(at(3))
        .border(px(2.0))
        .border_color(green)
        .shadow([BoxShadow { offset_x: px(0.0), offset_y: px(1.0), blur: px(6.0), spread: px(0.0), color: nsh_core::Color::rgba(0, 0, 0, 64), inset: false }])
        .bg(move || if u.chart_drag.with(Option::is_some) { tint } else { clear })
        .opacity(move || if rect.with(Option::is_some) { 1.0f32 } else { 0.0 })
}

/// Over the hit layer's content (it scrolls with it): a view on each chart
/// in view, with the web's cursor (`move`, or `pointer` for one that can't
/// move) and the drag; one on each link, with the pointer and its tip.
/// Neither takes presses: the hit layer's box hit-tests them itself.
fn objects(u: &'static SheetUi) -> impl IntoElement {
    // content points: the area's, plus the scroll
    let content = move |r: (f32, f32, f32, f32)| {
        let (sx, sy) = u.scroll.get();
        let z = u.zoom.get();
        (r.0 + sx * z, r.1 + sy * z, r.2.max(0.0), r.3.max(0.0))
    };
    let charts = computed(move || u.shown.with(|s| s.as_ref().map(|s| s.list.charts.iter().map(|c| (c.id.clone(), c.label.clone(), c.movable)).collect::<Vec<_>>()).unwrap_or_default()));
    let links = computed(move || u.shown.with(|s| s.as_ref().map(|s| s.list.hyperlinks.iter().map(|h| ((h.top, h.left), (h.bottom, h.right), h.tooltip.clone().unwrap_or_default())).collect::<Vec<_>>()).unwrap_or_default()));
    let placed = move |v: View, at: std::rc::Rc<dyn Fn() -> (f32, f32, f32, f32)>| {
        let (a, b, c, d) = (at.clone(), at.clone(), at.clone(), at);
        v.absolute().left(move || px(a().0)).top(move || px(b().1)).w(move || px(c().2)).h(move || px(d().3))
    };
    let link_views = dynamic(move || {
        let views = links.get().into_iter().map(|(from, to, tip)| {
            let at = std::rc::Rc::new(move || {
                let Some((shape, f)) = frame(u) else { return (0.0, 0.0, 0.0, 0.0) };
                let g = &shape.0.geometry;
                let (x0, y0, _, _) = cell_rect(g, &f, from);
                let (x1, y1, w1, h1) = cell_rect(g, &f, to);
                content((x0, y0, x1 + w1 - x0, y1 + h1 - y0))
            });
            let v = placed(view(), at).cursor(Cursor::Pointer);
            if tip.is_empty() { v.into_element() } else { v.tooltip(tip_view(tip)).into_element() }
        });
        view().absolute().left(px(0.0)).top(px(0.0)).children(views.collect::<Vec<_>>()).into_element()
    });
    let chart_views = dynamic(move || {
        let views = charts.get().into_iter().map(|(id, label, movable)| {
            let id2 = id.clone();
            let at = std::rc::Rc::new(move || chart::hit_rect(u, &id2).map_or((0.0, 0.0, 0.0, 0.0), content));
            let (moved, dropped) = (id.clone(), id.clone());
            let delta = |e: &ResponderEvent| (e.gesture.delta.x.0, e.gesture.delta.y.0);
            placed(view(), at)
                .label(if label.is_empty() { "Chart".to_owned() } else { label })
                .cursor(if movable { Cursor::Move } else { Cursor::Pointer })
                // a movable chart takes the drag: the outline follows, the
                // release moves it
                .claim_touches(TouchClaim::on_start().ask(move |r: &ClaimRequest| {
                    if movable {
                        r.claim();
                    }
                }))
                .keep_touches(true)
                .on_responder_move(move |e: &ResponderEvent| chart::drag(&moved, delta(e)))
                .on_responder_release(move |e: &ResponderEvent| chart::drop_at(&dropped, delta(e)))
                .on_responder_terminate(move |_: &ResponderEvent| chart::cancel_drag())
                .into_element()
        });
        view().absolute().left(px(0.0)).top(px(0.0)).children(views.collect::<Vec<_>>()).into_element()
    });
    view().absolute().left(px(0.0)).top(px(0.0)).child(link_views).child(chart_views)
}

/// A tip as the pointer rests: a small dark label.
fn tip_view(t: String) -> AnyElement {
    view().px(px(6.0)).py(px(3.0)).rounded(px(4.0)).bg(theme::TEXT).child(text(t).text_size(px(11.0)).text_color(theme::PANE)).into_element()
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
    let link_from = std::rc::Rc::new(std::cell::Cell::new(None::<(u32, u32)>));
    let link_from2 = link_from.clone();
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
                        .label("Cells")
                        .w(move || px(size().0))
                        .h(move || px(size().1))
                        .on_press_down(move |e: &PressEvent| {
                            let (ax, ay) = area_point(e.position.x.0, e.position.y.0);
                            link_from.set(None);
                            // a chart over the cells takes the press first
                            if let Some(region) = chart::at(u, ax, ay) {
                                d1.set(false);
                                chart::press(&region);
                                return;
                            }
                            let Some(cell) = cell_of(e.position.x.0, e.position.y.0) else { return };
                            // a click (down and up on one cell) follows its
                            // link, unless it ends an edit
                            if u.editing.with_untracked(Option::is_none) {
                                link_from.set(Some(cell));
                            }
                            d1.set(true);
                            edit::press(cell, e.modifiers.shift);
                        })
                        .on_press_up(move |_: &PressEvent| d2.set(false))
                        .on_press(move |e: &PressEvent| {
                            let (ax, ay) = area_point(e.position.x.0, e.position.y.0);
                            if chart::at(u, ax, ay).is_some() {
                                return;
                            }
                            let Some(cell) = cell_of(e.position.x.0, e.position.y.0) else { return };
                            if e.clicks == 1 && link_from2.take() == Some(cell) {
                                edit::follow_link(cell);
                            } else if e.clicks == 2 && edit::link_at(cell).is_none() {
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
                            // none over a chart, as on the web
                            let (ax, ay) = area_point(e.position.x.0, e.position.y.0);
                            if chart::at(u, ax, ay).is_some() {
                                return;
                            }
                            if let Some(cell) = cell_of(e.position.x.0, e.position.y.0) {
                                edit::menu(cell, e.page_position.x.0, e.page_position.y.0);
                            }
                        })
                        .child(objects(u)),
                ),
        )
}

/// The column (`cols`) or row a point along a header is on.
fn header_index(u: &SheetUi, cols: bool, at: f32) -> Option<u32> {
    let (shape, f) = untracked(|| frame(u))?;
    let g = &shape.0.geometry;
    let (x, y) = f.to_sheet(at, at);
    Some(if cols { g.col_at_x(x).min(LAST_COL) } else { g.row_at_y(y).min(LAST_ROW) })
}

/// The headers take presses (a whole column or row), the menu, and on a
/// border its resize (Excel's): the band under the pointer there.
fn header_hits(u: &'static SheetUi, cols: bool) -> View {
    // the border the pointer is on
    let edge = signal(None::<u32>);
    let along = move |e: &MouseEvent| if cols { e.position.x.0 } else { e.position.y.0 };
    let band_at = move || -> f32 {
        let Some(i) = edge.get() else { return -100.0 };
        let Some((shape, f)) = frame(u) else { return -100.0 };
        let g = &shape.0.geometry;
        (if cols { f.screen_x(g.col_x(i + 1)) } else { f.screen_y(g.row_y(i + 1)) }) - resize::BAND
    };
    let held = move || edge.get_untracked();
    let delta = move |e: &ResponderEvent| if cols { e.gesture.delta.x.0 } else { e.gesture.delta.y.0 };
    let band = view()
        .label(if cols { "Resize column" } else { "Resize row" })
        .absolute()
        .cursor(if cols { Cursor::ColResize } else { Cursor::RowResize })
        .pointer_events(move || if edge.with(Option::is_some) { PointerEvents::Auto } else { PointerEvents::None })
        .claim_touches(TouchClaim::on_start().ask(move |r: &ClaimRequest| {
            if held().is_some() {
                r.claim();
            }
        }))
        .keep_touches(true)
        .on_responder_grant(move |_: &ResponderEvent| {
            if let Some(i) = held() {
                resize::start(cols, i);
            }
        })
        .on_responder_move(move |e: &ResponderEvent| resize::drag(delta(e)))
        .on_responder_release(move |e: &ResponderEvent| resize::drop_at(delta(e)))
        .on_responder_terminate(move |_: &ResponderEvent| resize::cancel())
        .on_press(move |e: &PressEvent| {
            if e.clicks == 2
                && let Some(i) = held()
            {
                resize::autofit(cols, i);
            }
        });
    let band = if cols { band.top(px(0.0)).bottom(px(0.0)).w(px(resize::BAND * 2.0)).left(move || px(band_at())) } else { band.left(px(0.0)).right(px(0.0)).h(px(resize::BAND * 2.0)).top(move || px(band_at())) };
    view()
        .label(if cols { "Column headers" } else { "Row headers" })
        .absolute()
        .size_full()
        .on_mouse_move(move |e: &MouseEvent| {
            // the band stays where a drag began
            if u.resize.with_untracked(Option::is_some) {
                return;
            }
            let Some((shape, f)) = untracked(|| frame(u)) else { return };
            let at = resize::border_at(&shape.0.geometry, &f, cols, along(e));
            if edge.get_untracked() != at {
                edge.set(at);
            }
        })
        .on_mouse_leave(move |_: &MouseEvent| {
            if u.resize.with_untracked(Option::is_none) && edge.get_untracked().is_some() {
                edge.set(None);
            }
        })
        .on_press_down(move |e: &PressEvent| {
            let at = if cols { e.position.x.0 } else { e.position.y.0 };
            let Some(i) = header_index(u, cols, at) else { return };
            let s = u.sel.get_untracked();
            let next = if cols { s.col(i, e.modifiers.shift) } else { s.row(i, e.modifiers.shift) };
            chart::deselect();
            edit::commit_editing();
            u.sel.set(next);
            edit::read_active();
        })
        .on_context_menu(move |e: &ContextMenuEvent| {
            let at = if cols { e.position.x.0 } else { e.position.y.0 };
            if let Some(i) = header_index(u, cols, at) {
                edit::header_menu(cols, i, e.page_position.x.0, e.page_position.y.0);
            }
        })
        .child(band)
}

/// A border drag's guide over the cells, and Excel's tip by the header.
fn resize_guide(u: &'static SheetUi) -> impl IntoElement {
    // the dragged border on screen, and the tip
    let at = computed(move || -> Option<(bool, f32, String)> {
        let d = u.resize.get()?;
        let (shape, f) = frame(u)?;
        let g = &shape.0.geometry;
        let size = resize::dragged_px(&d, f.zoom);
        let start = if d.cols { f.screen_x(g.col_x(d.index)) } else { f.screen_y(g.row_y(d.index)) };
        Some((d.cols, start + size * f.zoom, resize::tip(d.cols, size)))
    });
    let cols = move || at.with(|a| a.as_ref().is_none_or(|a| a.0));
    let pos = move || at.with(|a| a.as_ref().map_or(-10.0, |a| a.1));
    let down = view().absolute().pointer_events(PointerEvents::None).bg(theme::ACCENT).top(px(0.0)).bottom(px(0.0)).w(px(1.0)).left(move || px(pos() - 0.5)).opacity(move || if cols() { 1.0f32 } else { 0.0 });
    let across = view().absolute().pointer_events(PointerEvents::None).bg(theme::ACCENT).left(px(0.0)).right(px(0.0)).h(px(1.0)).top(move || px(pos() - 0.5)).opacity(move || if cols() { 0.0f32 } else { 1.0 });
    let tip = view()
        .absolute()
        .pointer_events(PointerEvents::None)
        .left(move || px(if cols() { pos() + 6.0 } else { 6.0 }))
        .top(move || px(if cols() { 4.0 } else { pos() + 6.0 }))
        .child(dynamic(move || match at.with(|a| a.as_ref().map(|a| a.2.clone())) {
            Some(t) => tip_view(t),
            None => view().into_element(),
        }));
    view()
        .absolute()
        .size_full()
        .pointer_events(PointerEvents::None)
        .opacity(move || if at.with(Option::is_some) { 1.0f32 } else { 0.0 })
        .child(down)
        .child(across)
        .child(tip)
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
                        .child(chart_outline(u))
                        .child(hits(u))
                        .child(edit::cell_editor(u))
                        .child(resize_guide(u)),
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
    fn the_tint_leaves_the_active_cell_out() {
        let s = SelRects { range: (0.0, 0.0, 100.0, 60.0), active: (20.0, 20.0, 40.0, 40.0), single: false };
        let bands = tint_bands(&s);
        assert_eq!(bands, [(0.0, 0.0, 100.0, 20.0), (0.0, 40.0, 100.0, 20.0), (0.0, 20.0, 20.0, 20.0), (40.0, 20.0, 60.0, 20.0)]);
        // the bands and the cell cover the range exactly
        let area: f32 = bands.iter().map(|b| b.2 * b.3).sum();
        assert_eq!(area + 20.0 * 20.0, 100.0 * 60.0);
        // a single cell is outlined, not tinted
        assert!(tint_bands(&SelRects { single: true, ..s }).iter().all(|b| b.2 * b.3 == 0.0));
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
