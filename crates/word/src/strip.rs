//! The page-thumbnail strip: a column beside the pages, each page a small
//! drawing with its number. The page at the top of the view is marked, a
//! press (or Enter or Space on the focused thumbnail) scrolls the document
//! to that page, and the strip follows the document's scroll so the marked
//! thumbnail stays in view.
//!
//! Like the pages, the thumbnails are views positioned in a scroll, one per
//! page near the strip's view (`each` keyed by page). Each is drawn once by
//! the worker, from the page's display list at [`THUMB_W`] (no second
//! layout), and a scroll only moves the views and asks for thumbnails
//! coming near: it never draws one. A key does not ask at all; the worker
//! draws again only the thumbnails whose page changed (`doc.rs`).

use nsh_core::AccessibilityCurrent;
use nsh_core::prelude::*;

use super::WordUi;
use super::doc::{Cmd, THUMB_W};
use super::view::{self, send};
use buwiz_office_kit::host::theme;

/// Space at either side of a thumbnail, and above and below its cell.
const SIDE: f32 = 12.0;
const ROW_PAD: f32 = 6.0;
/// The page number's line, and the marker's width around the thumbnail.
const LABEL_H: f32 = 16.0;
const MARK: f32 = 2.0;
/// How far past the strip's view thumbnails are kept, in view heights.
const AHEAD: f32 = 0.5;
/// The strip's width.
pub const WIDTH: f32 = THUMB_W + 2.0 * SIDE;

thread_local! {
    static STRIP: std::cell::Cell<Option<ElementRef>> = const { std::cell::Cell::new(None) };
}

fn ui() -> &'static WordUi {
    crate::FORMAT.ui()
}

/// A thumbnail's height for a page of `(w, h)`.
pub fn thumb_h((w, h): (f32, f32)) -> f32 {
    THUMB_W * h / w.max(1.0)
}

fn cell_h(page: (f32, f32)) -> f32 {
    ROW_PAD + MARK + thumb_h(page) + MARK + LABEL_H + ROW_PAD
}

/// Each page's cell top in the strip, and the strip's whole height.
pub fn cells(pages: &[(f32, f32)]) -> (Vec<f32>, f32) {
    let mut y = 0.0;
    let mut out = Vec::with_capacity(pages.len());
    for p in pages {
        out.push(y);
        y += cell_h(*p);
    }
    (out, y)
}

/// The pages whose cells are near the strip's view: first and last.
pub fn near(pages: &[(f32, f32)], scroll: f32, view_h: f32) -> Option<(usize, usize)> {
    if pages.is_empty() {
        return None;
    }
    let (tops, _) = cells(pages);
    let (lo, hi) = (scroll - view_h * AHEAD, scroll + view_h * (1.0 + AHEAD));
    let first = tops.iter().zip(pages).position(|(t, p)| t + cell_h(*p) >= lo).unwrap_or(0);
    let last = tops.iter().rposition(|t| *t <= hi).unwrap_or(first).max(first);
    Some((first, last))
}

/// The page the document's scroll is at: the last whose top is within a
/// third of the view (at most 120 points) of the view's top, and the last
/// page once the scroll is at the end (it can't reach a short last page's
/// top). `tops` and `total` are the pages' ([`view::tops`]).
pub fn current_of(tops: &[f32], total: f32, scroll: f32, view_h: f32) -> usize {
    if tops.is_empty() {
        return 0;
    }
    if view_h > 0.0 && scroll + view_h >= total - 1.0 && scroll > 0.0 {
        return tops.len() - 1;
    }
    let line = scroll + (view_h / 3.0).min(120.0);
    tops.iter().rposition(|t| *t <= line).unwrap_or(0)
}

/// The current page now.
pub fn current(u: &WordUi) -> usize {
    let z = u.zoom.get_untracked();
    let meta = u.meta.get_untracked();
    let (tops, total) = view::tops(&meta.pages, z);
    current_of(&tops, total, u.scroll.get_untracked(), u.area.get_untracked().1)
}

/// Whether the strip shows: as chosen, else when the document has more
/// than one page.
pub fn shown(u: &WordUi) -> bool {
    u.strip.get().unwrap_or_else(|| u.meta.with(|m| m.pages.len() > 1))
}

/// View ▸ Page thumbnails: shows the strip or hides it.
pub fn toggle() {
    let u = ui();
    let now = untracked(|| shown(u));
    u.strip.set(Some(!now));
    // shown again: it follows the document's scroll from the start
    u.strip_followed.set(None);
    ask(u);
}

/// Asks the worker for the thumbnails near the strip's view; none once the
/// strip is hidden. Said once for the same range, page count and layout:
/// a key never asks.
pub fn ask(u: &WordUi) {
    if !untracked(|| shown(u)) {
        if u.thumbs_asked.take().is_some() {
            send(Cmd::Thumbs { pages: Vec::new() });
        }
        return;
    }
    let meta = u.meta.get_untracked();
    let Some((first, last)) = near(&meta.pages, u.strip_scroll.get_untracked(), u.strip_area.get_untracked().1) else { return };
    let key = (first, last, meta.pages.len(), !meta.partial);
    if u.thumbs_asked.get() == Some(key) {
        return;
    }
    u.thumbs_asked.set(Some(key));
    send(Cmd::Thumbs { pages: (first..=last).collect() });
}

/// Scrolls the strip so the current page's thumbnail is in view, when the
/// current page has changed since it last did (a strip the person scrolled
/// stays where it is until the document moves to another page).
pub fn follow(u: &WordUi) {
    if !untracked(|| shown(u)) {
        return;
    }
    let cur = current(u);
    if u.strip_followed.get() == Some(cur) {
        return;
    }
    u.strip_followed.set(Some(cur));
    let meta = u.meta.get_untracked();
    let (tops, _) = cells(&meta.pages);
    let Some((top, page)) = tops.get(cur).zip(meta.pages.get(cur)) else { return };
    let bottom = top + cell_h(*page);
    let (_, h) = u.strip_area.get_untracked();
    let s = u.strip_scroll.get_untracked();
    let to = if *top < s {
        *top
    } else if bottom > s + h {
        bottom - h
    } else {
        return;
    };
    scroll_to(u, to.max(0.0));
}

fn scroll_to(u: &WordUi, to: f32) {
    if let Some(r) = STRIP.with(std::cell::Cell::get) {
        r.scroll_to(point(px(0.0), px(to)), false);
    }
    u.strip_scroll.set(to);
    ask(u);
}

/// One page's cell: its drawing in a frame (the marker's colour while it is
/// the current page, clear otherwise: nothing moves) and its number.
fn thumb(i: ReadSignal<usize>, cur: Memo<usize>, geo: Memo<(Vec<f32>, f32)>) -> impl IntoElement {
    let u = ui();
    let page = i.get_untracked();
    let size = move || u.meta.with(|m| m.pages.get(i.get()).map_or((THUMB_W, THUMB_W * 1.3), |p| (THUMB_W, thumb_h(*p))));
    let top = move || geo.with(|g| g.0.get(i.get()).copied().unwrap_or(0.0));
    let on = move || cur.get() == i.get();
    let canvas = canvas(move || match u.thumb(i.get()).get() {
        Some(t) => Painter::from(t.0.drawing.clone()),
        None => Painter::new(),
    })
    .absolute()
    .left(px(0.0))
    .top(px(0.0))
    .w(px(THUMB_W))
    .h(move || px(size().1))
    .pointer_events(PointerEvents::None);
    view()
        .absolute()
        .left(px(0.0))
        .top(move || px(top()))
        .w(px(WIDTH))
        .h(move || px(ROW_PAD + MARK + size().1 + MARK + LABEL_H + ROW_PAD))
        .items_center()
        .py(px(ROW_PAD))
        .label(format!("Go to page {}", page + 1))
        .role(AccessibilityRole::Button)
        .accessibility_state(move || AccessibilityState { current: on().then_some(AccessibilityCurrent::Page), ..Default::default() })
        .on_press(move || view::go_to_page(i.get_untracked()))
        .child(
            view()
                .label(format!("Page {} thumbnail", page + 1))
                .w(px(THUMB_W + 2.0 * MARK))
                .h(move || px(size().1 + 2.0 * MARK))
                .border(px(MARK))
                .border_color(move || if on() { theme().accent } else { AdaptiveColor::TRANSPARENT })
                .child(view().size_full().bg(Hsla { h: 0.0, s: 0.0, l: 1.0, a: 1.0 }).overflow_hidden().child(canvas)),
        )
        .child(
            text(format!("{}", page + 1))
                .h(px(LABEL_H))
                .text_size(px(11.0))
                .text_color(move || if on() { theme().accent } else { theme().muted }),
        )
}

fn strip_view() -> impl IntoElement {
    let u = ui();
    let scroller = element_ref();
    STRIP.with(|s| s.set(Some(scroller)));
    let geo = computed(move || u.meta.with(|m| cells(&m.pages)));
    // the page at the top of the view
    let cur = computed(move || {
        let (z, s, h) = (u.zoom.get(), u.scroll.get(), u.area.get().1);
        u.meta.with(|m| {
            let (tops, total) = view::tops(&m.pages, z);
            current_of(&tops, total, s, h)
        })
    });
    // the thumbnails near the view, by page: one that stays keeps its views
    let visible = move || -> Vec<usize> {
        let _ = (u.strip_scroll.get(), u.strip_area.get(), u.meta.with(|m| m.pages.len()));
        untracked(|| near(&u.meta.get_untracked().pages, u.strip_scroll.get_untracked(), u.strip_area.get_untracked().1)).map(|(a, b)| (a..=b).collect()).unwrap_or_default()
    };
    scroll()
        .element_ref(&scroller)
        .label("Page thumbnail strip")
        .w(px(WIDTH))
        .bg(theme().pane)
        .border_r(px(0.5))
        .border_r_color(theme().border_s)
        .on_layout(move |b: &Bounds<Points>| {
            u.strip_area.set((b.size.width.0, b.size.height.0));
            ask(u);
            follow(u);
        })
        .on_scroll(move |e: &ScrollEvent| {
            u.strip_scroll.set(e.offset.y.0);
            ask(u);
        })
        .child(view().w(px(WIDTH)).h(move || px(geo.with(|g| g.1))).child(each(visible, |i: &usize| *i, move |i, _| thumb(i, cur, geo))))
}

/// The strip, while it shows.
pub fn strip() -> impl IntoElement {
    let u = ui();
    when(move || shown(u), strip_view)
}
