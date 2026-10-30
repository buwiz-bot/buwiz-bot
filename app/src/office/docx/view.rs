//! The Word view: a toolbar, the pages, a thumbnail strip, the review list
//! and the status bar.
//!
//! The pages are a scroll of canvases, one per page near the view (`each`
//! keyed by page: a page that stays keeps its canvas). The worker draws a
//! page once per layout and zoom (`doc.rs`); a scroll only moves the page
//! views and asks for pages coming near, so scrolling never repaints a
//! page's canvas. The caret, the selection and find's matches are views
//! over the pages, moved rather than drawn; pictures are image views.
//!
//! Typing goes into one hidden text input that keeps the focus; the keys
//! that edit (arrows, Delete, Return, ⌘B, …) are bound in `keys.rs`
//! (`$document`) and come here.

use nsh_base::input;
use nsh_core::prelude::*;

use super::WordUi;
use super::doc::{Cmd, Fmt, Move};
use crate::live::store;
use crate::office;
use crate::theme;

/// Space around and between pages, points.
const GAP: f32 = 16.0;
/// How far past the view pages are kept drawn, in view heights.
const AHEAD: f32 = 1.0;

fn ui() -> &'static WordUi {
    &store().office.word
}

fn send(cmd: Cmd) {
    office::with_word(|s| s.send(cmd));
}

thread_local! {
    static FIELD: std::cell::Cell<Option<ElementRef>> = const { std::cell::Cell::new(None) };
    static SCROLLER: std::cell::Cell<Option<ElementRef>> = const { std::cell::Cell::new(None) };
}

/// Whether the document's keys apply: it shows, and no menu, prompt or
/// find bar has the keyboard.
pub(crate) fn editing() -> bool {
    let st = store();
    office::showing() && st.office.is_word.get() && st.office.word.find.with(Option::is_none) && !st.office.ask_close.get() && st.ctx.get().is_none()
}

/// The keyboard back to the page.
pub(crate) fn focus() {
    if let Some(f) = FIELD.with(std::cell::Cell::get) {
        f.focus();
    }
}

/// Each page's top, and the whole height, at `zoom` (points).
fn tops(pages: &[(f32, f32)], zoom: f32) -> (Vec<f32>, f32) {
    let mut y = GAP;
    let mut out = Vec::with_capacity(pages.len());
    for (_, h) in pages {
        out.push(y);
        y += h * zoom + GAP;
    }
    (out, y)
}

/// The pages near the view: first and last.
fn near(u: &WordUi) -> Option<(usize, usize)> {
    let meta = u.meta.get_untracked();
    if meta.pages.is_empty() {
        return None;
    }
    let z = u.zoom.get_untracked();
    let (tops, _) = tops(&meta.pages, z);
    let (_, h) = u.area.get_untracked();
    let s = u.scroll.get_untracked();
    let (lo, hi) = (s - h * AHEAD, s + h * (1.0 + AHEAD));
    let first = tops.iter().zip(&meta.pages).position(|(t, p)| t + p.1 * z >= lo).unwrap_or(0);
    let last = tops.iter().rposition(|t| *t <= hi).unwrap_or(first).max(first);
    Some((first, last))
}

/// Asks the worker for the pages near the view (`again`: whatever was
/// asked before, after a relayout).
pub(crate) fn ask_pages(u: &WordUi, again: bool) {
    let Some((first, last)) = near(u) else { return };
    let z = u.zoom.get_untracked();
    let rev = u.meta.with_untracked(|m| m.rev);
    if !again && u.asked.get() == Some((first, last, z, rev)) {
        return;
    }
    u.asked.set(Some((first, last, z, rev)));
    send(Cmd::View { pages: (first..=last).collect(), zoom: z });
}

/// Scrolls page `page`'s `y` (page pixels) into view, if it is out.
pub(crate) fn reveal(u: &WordUi, page: usize, y: f32) {
    let meta = u.meta.get_untracked();
    let z = u.zoom.get_untracked();
    let (tops, _) = tops(&meta.pages, z);
    let Some(top) = tops.get(page) else { return };
    let at = top + y * z;
    let (_, h) = u.area.get_untracked();
    let s = u.scroll.get_untracked();
    if at >= s + 40.0 && at <= s + h - 60.0 {
        return;
    }
    let to = (at - h / 3.0).max(0.0);
    if let Some(r) = SCROLLER.with(std::cell::Cell::get) {
        r.scroll_to(point(px(0.0), px(to)), false);
    }
    u.scroll.set(to);
    ask_pages(u, false);
}

/// Scrolls to the top of page `page`.
fn go_to_page(page: usize) {
    let u = ui();
    let meta = u.meta.get_untracked();
    let (tops, _) = tops(&meta.pages, u.zoom.get_untracked());
    let Some(top) = tops.get(page) else { return };
    let to = (top - GAP).max(0.0);
    if let Some(r) = SCROLLER.with(std::cell::Cell::get) {
        r.scroll_to(point(px(0.0), px(to)), false);
    }
    u.scroll.set(to);
    ask_pages(u, false);
}

/// A key's move.
pub(crate) fn step(to: Move, extend: bool) {
    send(Cmd::Move { to, extend });
}

pub(crate) fn key(cmd: &str) {
    send(match cmd {
        "backspace" => Cmd::Backspace,
        "delete" => Cmd::Delete,
        "enter" => Cmd::Enter,
        "tab" => Cmd::Tab,
        "all" => Cmd::SelectAll,
        "undo" => Cmd::Undo,
        "redo" => Cmd::Redo,
        "row-above" => Cmd::Row(Some(false)),
        "row-below" => Cmd::Row(Some(true)),
        "row-delete" => Cmd::Row(None),
        _ => return,
    });
}

pub(crate) fn format(f: Fmt) {
    send(Cmd::Format(f));
}

pub(crate) fn copy(cut: bool) {
    send(Cmd::Copy { cut });
}

pub(crate) fn paste() {
    let answer = clipboard_string();
    crate::store::spawn_detached(async move {
        let text = answer.await;
        if !text.is_empty() {
            send(Cmd::Paste(text));
        }
    });
}

/// ⌘P: the document as it is now, to the print panel.
pub(crate) fn print() {
    send(Cmd::Print);
}

/// ⌘S on a `.doc` on this Mac: where to write the `.docx` first.
pub(crate) fn save_legacy(path: std::path::PathBuf) {
    let name = path.with_extension("docx").file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut dialog = nsh_core::file_dialog::SaveDialog::new().file_name(name).filter(nsh_core::file_dialog::FileFilter::new("Word Document", ["docx"]));
    if let Some(dir) = path.parent() {
        dialog = dialog.directory(dir);
    }
    let answer = pick_save_path(dialog);
    crate::store::spawn_detached(async move {
        match answer.await {
            Ok(Some(to)) => {
                send(Cmd::SaveTo(to.with_extension("docx")));
                office::save();
            }
            Ok(None) => {}
            Err(e) => ui().note.set(format!("Couldn't choose where to save: {e}")),
        }
    });
}

/// ⌘F: the find bar (again: the next match).
pub(crate) fn find() {
    let u = ui();
    if u.find.with_untracked(Option::is_none) {
        u.find.set(Some(String::new()));
    }
}

fn close_find() {
    let u = ui();
    u.find.set(None);
    send(Cmd::Find { query: String::new(), back: false });
    focus();
}

pub(crate) fn zoom_by(d: f32) {
    let u = ui();
    let z = ((u.zoom.get_untracked() + d) * 10.0).round() / 10.0;
    u.zoom.set(z.clamp(0.5, 3.0));
    ask_pages(u, true);
}

/// Escape: closes the viewer (asking about edits).
pub(crate) fn escape() {
    if office::may_close() {
        office::close();
        store().viewing.set(None);
    }
}

/// The hidden input typing goes into.
fn typing() -> impl IntoElement {
    let field = element_ref();
    FIELD.with(|f| f.set(Some(field.clone())));
    let buf = signal(String::new());
    input()
        .element_ref(&field)
        .label("Document")
        .auto_focus(true)
        .value(move || buf.get())
        .on_change(move |v: &str| {
            if v.is_empty() {
                return;
            }
            buf.set(v.to_owned());
            send(Cmd::Type(v.to_owned()));
            // emptied again once this change is through
            crate::store::spawn_detached(async move { buf.set(String::new()) });
        })
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .w(px(2.0))
        .h(px(2.0))
        .opacity(0.0f32)
}

/// A page: its canvas, its pictures, and the caret and selection on it.
fn page_view(i: ReadSignal<usize>) -> impl IntoElement {
    let u = ui();
    let shown = move || u.page(i.get()).get();
    let size = move || {
        let z = u.zoom.get();
        u.meta.with(|m| m.pages.get(i.get()).map_or((0.0, 0.0), |p| (p.0 * z, p.1 * z)))
    };
    let top = move || {
        let z = u.zoom.get();
        u.meta.with(|m| tops(&m.pages, z).0.get(i.get()).copied().unwrap_or(0.0))
    };
    let left = move || ((u.area.get().0 - size().0) / 2.0).max(GAP);
    let z = move || u.zoom.get();
    // the canvas: drawn once per layout and zoom (the worker's drawing)
    let page_canvas = canvas(move || match shown() {
        Some(s) => Painter::from(s.drawn.drawing.clone()),
        None => Painter::new(),
    })
    .absolute()
    .left(px(0.0))
    .top(px(0.0))
    .w(move || px(shown().map_or(0.0, |s| s.drawn.w)))
    .h(move || px(shown().map_or(0.0, |s| s.drawn.h)))
    .pointer_events(PointerEvents::None);
    let pictures = dynamic(move || {
        let Some(s) = shown() else { return view().into_element() };
        let media = u.media.get();
        let scale = s.zoom;
        let mut layer = view().absolute().size_full().pointer_events(PointerEvents::None);
        for p in &s.drawn.pictures {
            let Some(uri) = media.get(&(p.part.clone(), p.rel_id.clone())).cloned() else { continue };
            layer = layer.child(
                image(uri)
                    .object_fit(ResizeMode::Stretch)
                    .fade_duration(0.0)
                    .absolute()
                    .left(px(p.x * scale))
                    .top(px(p.y * scale))
                    .w(px(p.w * scale))
                    .h(px(p.h * scale))
                    .opacity(p.opacity),
            );
        }
        layer.into_element()
    });
    // the selection, find's matches and the caret: views that move
    let marks = dynamic(move || {
        let c = u.caret.get();
        let page = i.get();
        let z = z();
        let mut layer = view().absolute().size_full().pointer_events(PointerEvents::None);
        let found_tint = Hsla { h: 0.14, s: 0.95, l: 0.55, a: 0.35 };
        for (p, x, y, w, h) in c.found.iter().filter(|r| r.0 == page) {
            let _ = p;
            layer = layer.child(view().absolute().left(px(x * z)).top(px(y * z)).w(px(w * z)).h(px(h * z)).bg(found_tint));
        }
        for (_, x, y, w, h) in c.sel.iter().filter(|r| r.0 == page) {
            layer = layer.child(view().absolute().left(px(x * z)).top(px(y * z)).w(px(w * z)).h(px(h * z)).bg(theme::ACCENT_TINT));
        }
        if let Some((p, x, y, h)) = c.caret
            && p == page
            && c.sel.is_empty()
        {
            layer = layer.child(view().absolute().left(px(x * z - 0.75)).top(px(y * z)).w(px(1.5)).h(px(h * z)).bg(theme::ACCENT));
        }
        layer.into_element()
    });
    view()
        .absolute()
        .left(move || px(left()))
        .top(move || px(top()))
        .w(move || px(size().0))
        .h(move || px(size().1))
        // a page not drawn yet shows blank
        .bg(Hsla { h: 0.0, s: 0.0, l: 1.0, a: 1.0 })
        .border(px(0.5))
        .border_color(theme::BORDER_S)
        .overflow_hidden()
        .pointer_events(PointerEvents::None)
        .child(page_canvas)
        .child(pictures)
        .child(marks)
}

/// The page under a point of the pages' content: (page, x, y) in page pixels.
fn page_at(x: f32, y: f32) -> Option<(usize, f32, f32)> {
    let u = ui();
    let meta = u.meta.get_untracked();
    let z = u.zoom.get_untracked();
    let (tops, _) = tops(&meta.pages, z);
    let i = tops.iter().rposition(|t| *t <= y + GAP / 2.0)?;
    let (w, h) = meta.pages[i];
    let left = ((u.area.get_untracked().0 - w * z) / 2.0).max(GAP);
    Some((i, ((x - left) / z).clamp(0.0, w), ((y - tops[i]) / z).clamp(0.0, h)))
}

fn pages() -> impl IntoElement {
    let u = ui();
    let scroller = element_ref();
    SCROLLER.with(|s| s.set(Some(scroller.clone())));
    let total = move || {
        let z = u.zoom.get();
        u.meta.with(|m| tops(&m.pages, z).1)
    };
    let width = move || {
        let z = u.zoom.get();
        let widest = u.meta.with(|m| m.pages.iter().map(|p| p.0).fold(0.0f32, f32::max));
        (widest * z + 2.0 * GAP).max(u.area.get().0)
    };
    // the pages near the view, by index: one that stays keeps its views
    let visible = move || -> Vec<usize> {
        let _ = (u.scroll.get(), u.area.get(), u.zoom.get(), u.meta.with(|m| m.pages.len()));
        untracked(|| near(u)).map(|(a, b)| (a..=b).collect()).unwrap_or_default()
    };
    let dragging = std::rc::Rc::new(std::cell::Cell::new(false));
    let (d1, d2) = (dragging.clone(), dragging);
    let last = std::rc::Rc::new(std::cell::Cell::new((usize::MAX, 0.0f32, 0.0f32)));
    scroll()
        .element_ref(&scroller)
        .flex_1()
        .bg(theme::SUBTLE)
        .on_layout(move |b: &Bounds<Points>| {
            u.area.set((b.size.width.0, b.size.height.0));
            ask_pages(u, false);
        })
        .on_scroll(move |e: &ScrollEvent| {
            u.scroll.set(e.offset.y.0);
            ask_pages(u, false);
        })
        .child(
            view()
                .w(move || px(width()))
                .h(move || px(total()))
                .on_press_down(move |e: &PressEvent| {
                    focus();
                    let Some((page, x, y)) = page_at(e.position.x.0, e.position.y.0) else { return };
                    d1.set(true);
                    send(Cmd::Click { page, x, y, extend: e.modifiers.shift, clicks: e.clicks.min(3) as u8 });
                })
                .on_mouse_move(move |e: &MouseEvent| {
                    if !d2.get() || e.pressed_button.is_none() {
                        d2.set(false);
                        return;
                    }
                    let Some((page, x, y)) = page_at(e.position.x.0, e.position.y.0) else { return };
                    if last.get() != (page, x, y) {
                        last.set((page, x, y));
                        send(Cmd::Click { page, x, y, extend: true, clicks: 1 });
                    }
                })
                .child(each(visible, |i: &usize| *i, |i, _| page_view(i))),
        )
}

/// The thumbnail strip: each page small, from its drawing when it has one.
fn thumbs() -> impl IntoElement {
    let u = ui();
    const W: f32 = 96.0;
    let count = move || u.meta.with(|m| m.pages.len());
    let thumb = move |at: nsh_core::element::Position| {
        let i = at.index();
        let size = move || u.meta.with(|m| m.pages.get(i).map_or((W, W * 1.3), |p| (W, W * p.1 / p.0.max(1.0))));
        view()
            .items_center()
            .py(px(6.0))
            .child(
                view()
                    .w(move || px(size().0))
                    .h(move || px(size().1))
                    .bg(Hsla { h: 0.0, s: 0.0, l: 1.0, a: 1.0 })
                    .border(px(0.5))
                    .border_color(theme::BORDER_S)
                    .overflow_hidden()
                    .child(canvas(move || {
                        let Some(s) = u.page(i).get() else { return Painter::new() };
                        let k = W / s.drawn.w.max(1.0);
                        let mut d = Painter::new().scale(k, k).finish();
                        d.ops.extend(s.drawn.drawing.ops.iter().cloned());
                        Painter::from(d)
                    })
                    .size_full()),
            )
            .child(text(format!("{}", i + 1)).text_size(px(11.0)).text_color(theme::MUTED))
            .on_press(move || go_to_page(i))
    };
    view()
        .w(px(W + 24.0))
        .bg(theme::PANE)
        .border_r(px(0.5))
        .border_r_color(theme::BORDER_S)
        .child(virtual_list(count, thumb).flex_1())
}

/// The review list: tracked changes and comments, in their author's color.
fn review() -> impl IntoElement {
    let u = ui();
    dynamic(move || {
        let meta = u.meta.get();
        let mut list = view().gap_2().p_2();
        if meta.review.is_empty() {
            list = list.child(text("No tracked changes or comments").text_size(px(12.0)).text_color(theme::MUTED));
        }
        for (author, kind, body) in meta.review.iter() {
            let c = super::draw::author_color(author);
            list = list.child(
                view()
                    .gap_1()
                    .p_2()
                    .rounded(px(6.0))
                    .bg(theme::CARD)
                    .border_l(px(3.0))
                    .border_l_color(c)
                    .child(text(format!("{author} · {kind}")).text_size(px(11.0)).text_color(c))
                    .child(text(body.clone()).number_of_lines(4).text_size(px(12.0)).text_color(theme::TEXT)),
            );
        }
        scroll().w(px(240.0)).bg(theme::PANE).border_l(px(0.5)).border_l_color(theme::BORDER_S).child(list).into_element()
    })
}

fn tool(label: &'static str, title: &'static str, on: impl Fn() + 'static, active: impl Fn() -> bool + 'static) -> View {
    view()
        .label(title)
        .h(px(24.0))
        .min_w(px(26.0))
        .px(px(6.0))
        .rounded(px(5.0))
        .items_center()
        .justify_center()
        .bg(move || if active() { theme::ACTIVE } else { AdaptiveColor::TRANSPARENT })
        .child(text(label).text_size(px(13.0)).text_color(theme::TEXT))
        .on_press(move || {
            on();
            focus();
        })
}

fn toolbar() -> impl IntoElement {
    let u = ui();
    let m = move || u.meta.get();
    view()
        .flex_row()
        .items_center()
        .h(px(34.0))
        .px_2()
        .gap_1()
        .bg(theme::PANE)
        .border_b(px(0.5))
        .border_b_color(theme::BORDER_S)
        .child(tool("☰", "Pages", move || u.thumbs.update(|t| *t = !*t), move || u.thumbs.get()))
        .child(view().w(px(8.0)))
        .child(tool("B", "Bold", || format(Fmt::Bold), move || m().bold))
        .child(tool("I", "Italic", || format(Fmt::Italic), move || m().italic))
        .child(tool("U", "Underline", || format(Fmt::Underline), move || m().underline))
        .child(tool("A−", "Smaller", || format(Fmt::Size(false)), || false))
        .child(text(move || format!("{}", m().size)).w(px(30.0)).text_center().text_size(px(12.0)).text_color(theme::MUTED))
        .child(tool("A+", "Bigger", || format(Fmt::Size(true)), || false))
        .child(view().w(px(8.0)))
        .child(tool("•", "Bulleted list", || format(Fmt::Bullets), || false))
        .child(tool("1.", "Numbered list", || format(Fmt::Numbers), || false))
        .child(view().w(px(8.0)))
        .child(tool("↶", "Undo", || key("undo"), || false).opacity(move || if m().can_undo { 1.0f32 } else { 0.4 }))
        .child(tool("↷", "Redo", || key("redo"), || false).opacity(move || if m().can_redo { 1.0f32 } else { 0.4 }))
        .child(view().flex_1())
        .child(tool("Find", "Find", find, move || u.find.with(Option::is_some)))
        .child(tool("Review", "Tracked changes and comments", move || u.review.update(|r| *r = !*r), move || u.review.get()))
}

fn find_bar() -> impl IntoElement {
    let u = ui();
    when(
        move || u.find.with(Option::is_some),
        move || {
            let query = move || u.find.get().unwrap_or_default();
            let m = move || u.meta.get();
            view()
                .flex_row()
                .items_center()
                .h(px(34.0))
                .px_2()
                .gap_2()
                .bg(theme::PANE)
                .border_b(px(0.5))
                .border_b_color(theme::BORDER_S)
                .child(
                    input()
                        .label("Find")
                        .auto_focus(true)
                        .w(px(260.0))
                        .h(px(24.0))
                        .px_2()
                        .rounded(px(5.0))
                        .bg(theme::SUBTLE)
                        .text_size(px(13.0))
                        .text_color(theme::TEXT)
                        .placeholder("Find in document")
                        .value(query)
                        .on_change(move |v: &str| {
                            u.find.set(Some(v.to_owned()));
                            send(Cmd::Find { query: v.to_owned(), back: false });
                        })
                        .on_submit(move |v: &str| send(Cmd::Find { query: v.to_owned(), back: false }))
                        .on_key_press(move |k: &str| {
                            if k == "Escape" {
                                close_find();
                            }
                        }),
                )
                .child(text(move || match m().found {
                    (0, _) if !query().is_empty() => "No matches".to_owned(),
                    (0, _) => String::new(),
                    (n, at) => format!("{at} of {n}"),
                })
                .text_size(px(12.0))
                .text_color(theme::MUTED))
                .child(tool("‹", "Previous match", move || send(Cmd::Find { query: query(), back: true }), || false))
                .child(tool("›", "Next match", move || send(Cmd::Find { query: query(), back: false }), || false))
                .child(view().flex_1())
                .child(tool("Done", "Close find", close_find, || false))
                .into_element()
        },
    )
}

fn status_bar() -> impl IntoElement {
    let u = ui();
    let status = move || {
        let st = store().office.state.get();
        if !st.error.is_empty() {
            return st.error;
        }
        let note = u.note.get();
        if !note.is_empty() {
            return note;
        }
        if st.saving {
            return "Saving…".to_owned();
        }
        if u.meta.with(|m| m.partial) {
            return "Laying out…".to_owned();
        }
        if st.dirty { "Edited".to_owned() } else { String::new() }
    };
    let page = move || {
        let n = u.meta.with(|m| m.pages.len());
        let at = u.caret.with(|c| c.caret.map(|c| c.0)).unwrap_or(0);
        if n == 0 { String::new() } else { format!("Page {} of {n}", at + 1) }
    };
    let zoom_btn = |label: &'static str, d: f32| {
        view()
            .label(if d > 0.0 { "Zoom in" } else { "Zoom out" })
            .size(px(22.0))
            .rounded(px(5.0))
            .items_center()
            .justify_center()
            .child(text(label).text_size(px(13.0)).text_color(theme::MUTED))
            .on_press(move || zoom_by(d))
    };
    view()
        .flex_row()
        .items_center()
        .h(px(30.0))
        .px_2()
        .gap_2()
        .bg(theme::SUBTLE)
        .border_t(px(0.5))
        .border_t_color(theme::BORDER_S)
        .child(text(page).text_size(px(12.0)).text_color(theme::MUTED))
        .child(text(status).flex_1().number_of_lines(1).text_size(px(12.0)).text_color(theme::MUTED))
        .child(zoom_btn("−", -0.1))
        .child(text(move || format!("{}%", (u.zoom.get() * 100.0).round())).w(px(40.0)).text_center().text_size(px(12.0)).text_color(theme::MUTED))
        .child(zoom_btn("+", 0.1))
}

/// The Word view.
pub(crate) fn word_view() -> impl IntoElement {
    let u = ui();
    view()
        .flex_1()
        .child(office::xlsx::edit::prompts())
        .child(toolbar())
        .child(find_bar())
        .child(
            view()
                .flex_1()
                .flex_row()
                .child(when(move || u.thumbs.get(), thumbs))
                .child(view().flex_1().child(pages()).child(typing()))
                .child(when(move || u.review.get(), review)),
        )
        .child(status_bar())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_stack_with_gaps() {
        let (t, total) = tops(&[(816.0, 1056.0), (816.0, 1056.0)], 0.5);
        assert_eq!(t, vec![GAP, GAP + 528.0 + GAP]);
        assert_eq!(total, GAP + 2.0 * (528.0 + GAP));
    }
}
