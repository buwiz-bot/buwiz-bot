//! The Word view: the web editor's title bar and menus ([`super::chrome`]),
//! a toolbar, the pages and the review list. Its dialogs are
//! [`super::dialogs`]. Like the web there is no status bar: notes are
//! toasts.
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
use crate::office::ui::palette as p;
use crate::theme;

/// Space between pages (`CANVAS_PAGE_GAP_PX`) and around them
/// (`CANVAS_PAGES_PADDING_PX`), points.
const GAP: f32 = 16.0;
pub(crate) const PAD: f32 = 24.0;
/// How far past the view pages are kept drawn, in view heights.
const AHEAD: f32 = 1.0;

fn ui() -> &'static WordUi {
    &store().office.word
}

/// A command from the view. One that moves the caret or edits other than
/// by typing starts the hidden field afresh: what it held is behind the
/// caret no more.
pub(crate) fn send(cmd: Cmd) {
    if !matches!(cmd, Cmd::View { .. } | Cmd::Type(_) | Cmd::Print | Cmd::SaveTo(_) | Cmd::Copy { cut: false }) {
        empty_field();
    }
    post(cmd);
}

fn post(cmd: Cmd) {
    office::with_word(|s| s.send(cmd));
}

/// The hidden field typing goes into: its value, and the text it has
/// given the document since it was last emptied. A change carries the
/// field's whole text, earlier characters again when keys come faster
/// than it is emptied (key repeat, an input method, a paste, the test
/// agent): only what differs from what was given goes on.
#[derive(Default)]
struct Field {
    value: Option<Signal<String>>,
    given: String,
}

thread_local! {
    static FIELD: std::cell::Cell<Option<ElementRef>> = const { std::cell::Cell::new(None) };
    static SCROLLER: std::cell::Cell<Option<ElementRef>> = const { std::cell::Cell::new(None) };
    static TYPED: std::cell::RefCell<Field> = std::cell::RefCell::new(Field::default());
}

/// Empties the field and what it has given, together.
fn empty_field() {
    let value = TYPED.with_borrow_mut(|t| {
        t.given.clear();
        t.value
    });
    if let Some(v) = value {
        v.try_set(String::new());
    }
}

/// From `given` to `now`: the characters to delete back, then the text to
/// type.
fn delta(given: &str, now: &str) -> (usize, String) {
    let same = given.chars().zip(now.chars()).take_while(|(a, b)| a == b).count();
    (given.chars().count() - same, now.chars().skip(same).collect())
}

/// Whether the document's keys apply: it shows, and no menu, dialog,
/// prompt, find bar or the name field has the keyboard.
pub(crate) fn editing() -> bool {
    let st = store();
    let w = &st.office.word;
    office::showing()
        && st.office.is_word.get()
        && w.find.with(Option::is_none)
        && !w.naming.get()
        && !w.menu_open.get()
        && !w.setup_open.get()
        && !w.watermark_open.get()
        && !st.office.ask_close.get()
        && st.ctx.get().is_none()
}

/// The keyboard back to the page.
pub(crate) fn focus() {
    if let Some(f) = FIELD.with(std::cell::Cell::get) {
        f.focus();
    }
}

/// Each page's top, and the whole height, at `zoom` (points).
fn tops(pages: &[(f32, f32)], zoom: f32) -> (Vec<f32>, f32) {
    let mut y = PAD;
    let mut out = Vec::with_capacity(pages.len());
    for (i, (_, h)) in pages.iter().enumerate() {
        if i > 0 {
            y += GAP;
        }
        out.push(y);
        y += h * zoom;
    }
    (out, y + PAD)
}

/// A page's left edge in the pages' width: centred, at least the padding in.
pub(crate) fn page_left(area_w: f32, page_w: f32) -> f32 {
    ((area_w - page_w) / 2.0).max(PAD)
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
    TYPED.with_borrow_mut(|t| *t = Field { value: Some(buf), given: String::new() });
    input()
        .element_ref(&field)
        .label("Document")
        .auto_focus(true)
        .value(move || buf.get())
        .on_change(move |v: &str| {
            let given = TYPED.with_borrow_mut(|t| std::mem::replace(&mut t.given, v.to_owned()));
            // the value follows the field, so emptying it is a change
            buf.set(v.to_owned());
            let (back, new) = delta(&given, v);
            for _ in 0..back {
                post(Cmd::Backspace);
            }
            if !new.is_empty() {
                post(Cmd::Type(new));
            }
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
    let left = move || page_left(u.area.get().0, size().0);
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
            // a picture put in here carries its own `data:` URL
            let Some(uri) = media.get(&(p.part.clone(), p.rel_id.clone())).cloned().or_else(|| p.rel_id.starts_with("data:").then(|| p.rel_id.clone())) else { continue };
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
    // the web's canvas page: white, `0 1px 3px var(--doc-shadow)`
    let shadow = BoxShadow { offset_x: px(0.0), offset_y: px(1.0), blur: px(3.0), spread: px(0.0), color: color(p::SHADOW).into(), inset: false };
    view()
        .absolute()
        .left(move || px(left()))
        .top(move || px(top()))
        .w(move || px(size().0))
        .h(move || px(size().1))
        .label(format!("Page {}", i.get_untracked() + 1))
        .bg(Hsla { h: 0.0, s: 0.0, l: 1.0, a: 1.0 })
        .shadow(vec![shadow])
        .pointer_events(PointerEvents::None)
        .child(view().size_full().overflow_hidden().child(page_canvas).child(pictures).child(marks))
}

/// The page under a point of the pages' content: (page, x, y) in page pixels.
fn page_at(x: f32, y: f32) -> Option<(usize, f32, f32)> {
    let u = ui();
    let meta = u.meta.get_untracked();
    let z = u.zoom.get_untracked();
    let (tops, _) = tops(&meta.pages, z);
    let i = tops.iter().rposition(|t| *t <= y + GAP / 2.0)?;
    let (w, h) = meta.pages[i];
    let left = page_left(u.area.get_untracked().0, w * z);
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
        (widest * z + 2.0 * PAD).max(u.area.get().0)
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
        .bg(p::BG)
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
                .label("Document pages")
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

/// The Word view.
pub(crate) fn word_view() -> impl IntoElement {
    let u = ui();
    view()
        .flex_1()
        .child(office::xlsx::edit::prompts())
        .child(super::chrome::title_bar())
        .child(toolbar())
        .child(find_bar())
        .child(
            view()
                .flex_1()
                .flex_row()
                .child(view().flex_1().child(pages()).child(typing()))
                .child(when(move || u.review.get(), review)),
        )
        .child(super::dialogs::dialogs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_gives_only_what_differs() {
        assert_eq!(delta("", "a"), (0, "a".to_owned()));
        assert_eq!(delta("ab", "abc"), (0, "c".to_owned()));
        assert_eq!(delta("abc", "abd"), (1, "d".to_owned()));
        assert_eq!(delta("abc", "a"), (2, String::new()));
        assert_eq!(delta("née", "née"), (0, String::new()));
    }

    #[test]
    fn pages_stack_with_gaps() {
        // 24 around the pages, 16 between them, as the web's canvas pages
        let (t, total) = tops(&[(816.0, 1056.0), (816.0, 1056.0)], 0.5);
        assert_eq!(t, vec![24.0, 24.0 + 528.0 + 16.0]);
        assert_eq!(total, 24.0 + 528.0 + 16.0 + 528.0 + 24.0);
        assert_eq!(page_left(1000.0, 816.0), 92.0);
        assert_eq!(page_left(800.0, 816.0), 24.0);
    }
}
