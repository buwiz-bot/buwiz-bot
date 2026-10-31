//! Moving around and editing the sheet: the keyboard's commands
//! (`keys.rs` binds them), the cell editor over the active cell, the
//! formula bar, the clipboard, the context menu, and the chrome around the
//! grid (sheet tabs, zoom, what the document is doing).
//!
//! The cell editor is one text input that stays focused over the active
//! cell, invisible until typed into: a typed key replaces the cell (as in
//! Excel), F2, a double click or the formula bar edit it in place. While
//! nothing is edited the sheet's keys (arrows, Tab, Enter, ⌘C, …) are
//! bound over it; while something is, the input has them.

use nsh_base::input;
use nsh_core::prelude::*;

use super::doc::Cmd;
use super::grid;
use super::select::{self, Bounds, Dir, Sel, Step};
use super::{Editing, SheetUi};
use crate::live::store;
use crate::office;
use crate::theme;

fn ui() -> &'static SheetUi {
    &store().office.sheet
}

fn send(cmd: Cmd) {
    office::with_session(|s| s.send(cmd));
}

thread_local! {
    /// The cell editor, focused while the sheet shows.
    static FIELD: std::cell::Cell<Option<ElementRef>> = const { std::cell::Cell::new(None) };
}

/// The keyboard to the sheet (the cell editor).
pub(crate) fn focus() {
    if let Some(f) = FIELD.with(std::cell::Cell::get) {
        f.focus();
    }
}

/// Whether the sheet's keys apply: it shows and nothing is being edited.
pub(crate) fn navigating() -> bool {
    let st = store();
    office::showing() && st.office.sheet.editing.with(Option::is_none) && !st.office.ask_close.get() && st.ctx.get().is_none()
}

/// Whether a cell is being edited.
pub(crate) fn editing() -> bool {
    office::showing() && store().office.sheet.editing.with(Option::is_some)
}

/// What a move needs of the sheet: its used range, a page of rows, the
/// hidden rows and columns.
fn with_bounds<T>(f: impl FnOnce(&Bounds) -> T) -> Option<T> {
    let u = ui();
    let shape = u.shape.get_untracked()?;
    let g = shape.0.geometry.clone();
    let (_, h) = u.area.get_untracked();
    let row_h = (g.row_y(1) - g.row_y(0)).max(1.0) * u.zoom.get_untracked();
    let g2 = g.clone();
    let hidden_row = move |r: u32| g.row_y(r + 1) <= g.row_y(r);
    let hidden_col = move |c: u32| g2.col_x(c + 1) <= g2.col_x(c);
    let b = Bounds { used: shape.0.used, page: ((h / row_h) as u32).saturating_sub(1).max(1), hidden_row: &hidden_row, hidden_col: &hidden_col };
    Some(f(&b))
}

fn set_sel(sel: Sel) {
    let u = ui();
    u.sel.set(sel);
    grid::reveal_active(u);
    read_active();
}

/// Asks what the formula bar shows for the active cell.
pub(crate) fn read_active() {
    let u = ui();
    let sheet = u.sheet();
    let cell = u.sel.get_untracked().active;
    send(Cmd::Read { sheet, cell });
}

/// An arrow, ⌘-arrow or page key; with Shift it extends.
pub(crate) fn nav(dir: Dir, step: Step, shift: bool) {
    let s = ui().sel.get_untracked();
    if let Some(next) = with_bounds(|b| s.step(dir, step, shift, b)) {
        set_sel(next);
    }
}

/// Tab, Shift-Tab, Enter, Shift-Enter: walk the range or move.
pub(crate) fn walk(dir: Dir) {
    let s = ui().sel.get_untracked();
    if let Some(next) = with_bounds(|b| s.walk(dir, b)) {
        set_sel(next);
    }
}

pub(crate) fn select_all() {
    commit_editing();
    let u = ui();
    u.sel.set(u.sel.get_untracked().all());
    focus();
}

/// A press on `cell` (Shift extends; ⌘ opens its link).
pub(crate) fn press(cell: (u32, u32), shift: bool, cmd: bool) {
    let u = ui();
    commit_editing();
    if cmd && let Some(url) = link_at(cell) {
        store().open_link(&url);
        return;
    }
    u.sel.set(u.sel.get_untracked().press(cell, shift));
    read_active();
    focus();
}

/// The link of `cell`, if the list in view has one there.
fn link_at((r, c): (u32, u32)) -> Option<String> {
    ui().shown.with_untracked(|s| {
        s.as_ref()?.list.hyperlinks.iter().find(|h| (h.top..=h.bottom).contains(&r) && (h.left..=h.right).contains(&c)).and_then(|h| h.external_target.clone())
    })
}

/// Starts editing `cell`: with `typed` text replacing it, else its
/// content as the formula bar has it.
pub(crate) fn begin(cell: (u32, u32), typed: Option<String>) {
    // (before the editing engine is ready an edit waits for it)
    let u = ui();
    if u.shape.with_untracked(Option::is_none) {
        return;
    }
    let sheet = u.sheet();
    let replace = typed.is_some();
    let text = typed.unwrap_or_else(|| {
        let (s, c, input) = u.bar.get_untracked();
        if s == sheet && c == cell { input } else { String::new() }
    });
    if u.sel.get_untracked().active != cell {
        u.sel.set(Sel::at(cell));
    }
    u.editing.set(Some(Editing { sheet, cell, text, replace }));
    focus();
}

/// F2: edit the active cell in place.
pub(crate) fn edit_active() {
    begin(ui().sel.get_untracked().active, None);
}

/// Sends the edit in progress (nothing when its text is unchanged).
pub(crate) fn commit_editing() {
    let u = ui();
    let Some(e) = u.editing.get_untracked() else { return };
    u.editing.set(None);
    let (s, c, before) = u.bar.get_untracked();
    if s == e.sheet && c == e.cell && before == e.text {
        return;
    }
    u.bar.set((e.sheet, e.cell, e.text.clone()));
    send(Cmd::Edit { sheet: e.sheet, cell: e.cell, input: e.text });
}

/// Enter, Tab and their Shift twins while editing: commit, then move.
pub(crate) fn commit_and(dir: Dir) {
    commit_editing();
    walk(dir);
    focus();
}

/// Escape while editing: the cell keeps what it had.
pub(crate) fn cancel() {
    ui().editing.set(None);
    focus();
}

/// Escape on the sheet: a range shrinks to its active cell, then the
/// viewer closes (asking when there are edits).
pub(crate) fn escape() {
    let u = ui();
    let s = u.sel.get_untracked();
    if !s.is_single() {
        u.sel.set(Sel::at(s.active));
        return;
    }
    if office::may_close() {
        store().viewing.set(None);
    }
}

pub(crate) fn copy(cut: bool) {
    let u = ui();
    let (sheet, range) = (u.sheet(), u.sel.get_untracked().range());
    send(Cmd::Copy { sheet, range });
    if cut {
        send(Cmd::Clear { sheet, range });
    }
}

/// ⌘V: tab-separated text at the range's top left.
pub(crate) fn paste() {
    let u = ui();
    let (sheet, (at, _)) = (u.sheet(), u.sel.get_untracked().range());
    let answer = clipboard_string();
    crate::store::spawn_detached(async move {
        let text = answer.await;
        let rows = select::paste_rows(&text);
        if rows.is_empty() {
            return;
        }
        let (h, w) = (rows.len() as u32, rows.iter().map(Vec::len).max().unwrap_or(1) as u32);
        send(Cmd::Paste { sheet, at, rows });
        let u = ui();
        u.sel.set(Sel { anchor: at, focus: (at.0 + h - 1, at.1 + w - 1), active: at });
    });
}

pub(crate) fn clear() {
    let u = ui();
    send(Cmd::Clear { sheet: u.sheet(), range: u.sel.get_untracked().range() });
}

pub(crate) fn undo(redo: bool) {
    send(if redo { Cmd::Redo } else { Cmd::Undo });
}

/// The copied cells on the pasteboard as text and as an HTML table (a
/// paste into Numbers, Pages or a mail keeps the grid).
pub(crate) fn to_clipboard(tsv: &str, html: &str) {
    #[cfg(target_os = "macos")]
    {
        use nsh_platform_macos::objc2::{msg_send, rc::Retained, runtime::{AnyClass, AnyObject}};
        use nsh_platform_macos::objc2_foundation::NSString;
        let Some(class) = AnyClass::get(c"NSPasteboard") else { return };
        let (text, html) = (NSString::from_str(tsv), NSString::from_str(html));
        let (text_kind, html_kind) = (NSString::from_str("public.utf8-plain-text"), NSString::from_str("public.html"));
        // SAFETY: NSPasteboard's documented methods with arguments of their
        // types, on the main thread (the engine's).
        unsafe {
            let board: Retained<AnyObject> = msg_send![class, generalPasteboard];
            let _: isize = msg_send![&*board, clearContents];
            let _: bool = msg_send![&*board, setString: &*text, forType: &*text_kind];
            let _: bool = msg_send![&*board, setString: &*html, forType: &*html_kind];
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = html;
        set_clipboard(tsv.to_owned());
    }
}

/// The right-click menu over `cell`.
pub(crate) fn menu(cell: (u32, u32), x: f32, y: f32) {
    use crate::store::CtxItem;
    let u = ui();
    commit_editing();
    if !u.sel.get_untracked().contains(cell) {
        u.sel.set(Sel::at(cell));
        read_active();
    }
    let ((r0, c0), (r1, c1)) = u.sel.get_untracked().range();
    let rows = if r1 > r0 { format!("Delete {} Rows", r1 - r0 + 1) } else { "Delete Row".to_owned() };
    let cols = if c1 > c0 { format!("Delete {} Columns", c1 - c0 + 1) } else { "Delete Column".to_owned() };
    let items = vec![
        CtxItem::row("Cut", "copy", "sheet", "cut".into()),
        CtxItem::row("Copy", "copy", "sheet", "copy".into()),
        CtxItem::row("Paste", "copy", "sheet", "paste".into()),
        CtxItem::separator(),
        CtxItem::row("Insert Row Above", "plus", "sheet", "row-above".into()),
        CtxItem::row("Insert Row Below", "plus", "sheet", "row-below".into()),
        CtxItem::row("Insert Column Left", "plus", "sheet", "col-left".into()),
        CtxItem::row("Insert Column Right", "plus", "sheet", "col-right".into()),
        CtxItem::separator(),
        CtxItem::row(&rows, "trash", "sheet", "delete-rows".into()),
        CtxItem::row(&cols, "trash", "sheet", "delete-cols".into()),
        CtxItem::row("Clear Contents", "close", "sheet", "clear".into()),
    ];
    crate::ctx::open_at(x, y, items);
}

/// A row of the sheet's menu.
pub(crate) fn action(what: &str) {
    let u = ui();
    let sheet = u.sheet();
    let ((r0, c0), (r1, c1)) = u.sel.get_untracked().range();
    match what {
        "cut" => copy(true),
        "copy" => copy(false),
        "paste" => paste(),
        "clear" => clear(),
        "row-above" => send(Cmd::Rows { sheet, at: r0, count: 1 }),
        "row-below" => send(Cmd::Rows { sheet, at: r1 + 1, count: 1 }),
        "col-left" => send(Cmd::Cols { sheet, at: c0, count: 1 }),
        "col-right" => send(Cmd::Cols { sheet, at: c1 + 1, count: 1 }),
        "delete-rows" => send(Cmd::Rows { sheet, at: r0, count: -((r1 - r0 + 1) as i64) }),
        "delete-cols" => send(Cmd::Cols { sheet, at: c0, count: -((c1 - c0 + 1) as i64) }),
        _ => {}
    }
    focus();
}

/// The input over the active cell: invisible until something is typed.
pub(crate) fn cell_editor(u: &'static SheetUi) -> impl IntoElement {
    let field = element_ref();
    FIELD.with(|f| f.set(Some(field.clone())));
    let rect = move || -> (f32, f32, f32, f32) {
        let Some((shape, f)) = grid::frame(u) else { return (0.0, 0.0, 0.0, 0.0) };
        let cell = u.editing.with(|e| e.as_ref().map(|e| e.cell)).unwrap_or_else(|| u.sel.get().active);
        grid::cell_rect(&shape.0.geometry, &f, cell)
    };
    let shown = move || u.editing.with(Option::is_some);
    let z = move || u.zoom.get();
    input()
        .element_ref(&field)
        .label("Cell")
        .auto_focus(true)
        .value(move || u.editing.with(|e| e.as_ref().map(|e| e.text.clone()).unwrap_or_default()))
        .on_change(move |v: &str| match u.editing.get_untracked() {
            Some(_) => u.editing.update(|e| {
                if let Some(e) = e {
                    e.text = v.to_owned();
                }
            }),
            None if !v.is_empty() => begin(u.sel.get_untracked().active, Some(v.to_owned())),
            None => {}
        })
        .on_submit(move |_: &str| commit_and(Dir::Down))
        .on_key_press(move |k: &str| match k {
            "Escape" => cancel(),
            "ArrowUp" | "ArrowDown" if u.editing.with_untracked(|e| e.as_ref().is_some_and(|e| e.replace)) => commit_and(if k == "ArrowUp" { Dir::Up } else { Dir::Down }),
            _ => {}
        })
        .absolute()
        .left(move || px(rect().0))
        .top(move || px(rect().1))
        .min_w(move || px(rect().2.max(40.0)))
        .h(move || px(rect().3.max(18.0)))
        .px(px(3.0))
        .text_size(move || px(14.0 * z()))
        .text_color(theme::TEXT)
        .bg(theme::EDITOR)
        .border(px(2.0))
        .border_color(theme::ACCENT)
        .opacity(move || if shown() { 1.0f32 } else { 0.0 })
        // unseen, presses go through to the grid (a double click edits)
        .pointer_events(move || if shown() { PointerEvents::Auto } else { PointerEvents::None })
}

/// The formula bar: the active cell's address and its content, edited
/// there as in the cell.
fn formula_bar(u: &'static SheetUi) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .h(px(32.0))
        .px_2()
        .gap_2()
        .bg(theme::PANE)
        .border_b(px(0.5))
        .border_b_color(theme::BORDER_S)
        .child(
            text(move || select::address(&u.sel.get()))
                .w(px(96.0))
                .number_of_lines(1)
                .text_size(px(12.0))
                .text_color(theme::MUTED),
        )
        .child(text("ƒx").text_size(px(13.0)).text_color(theme::FAINT))
        .child(
            input()
                .label("Formula bar")
                .flex_1()
                .h(px(24.0))
                .px_2()
                .rounded(px(5.0))
                .bg(theme::SUBTLE)
                .text_size(px(13.0))
                .text_color(theme::TEXT)
                .value(move || match u.editing.get() {
                    Some(e) => e.text,
                    None => {
                        let (s, c, input) = u.bar.get();
                        if s == u.sheet() && c == u.sel.get().active { input } else { String::new() }
                    }
                })
                .on_change(move |v: &str| match u.editing.get_untracked() {
                    Some(_) => u.editing.update(|e| {
                        if let Some(e) = e {
                            e.text = v.to_owned();
                        }
                    }),
                    // (the bar's own text set again is no edit)
                    None if v != u.bar.with_untracked(|b| b.2.clone()) => {
                        let cell = u.sel.get_untracked().active;
                        u.editing.set(Some(Editing { sheet: u.sheet(), cell, text: v.to_owned(), replace: false }));
                    }
                    None => {}
                })
                .on_submit(move |_: &str| commit_and(Dir::Down))
                .on_key_press(move |k: &str| {
                    if k == "Escape" {
                        cancel()
                    }
                }),
        )
}

/// The bottom bar: sheet tabs, the zoom, and what the document is doing.
fn status_bar(u: &'static SheetUi) -> impl IntoElement {
    let tabs = dynamic(move || {
        let Some(shape) = u.shape.get() else { return view().into_element() };
        let active = shape.0.active;
        scroll()
            .horizontal()
            .flex_1()
            .child(
                view().flex_row().gap_1().items_center().h_full().children(
                    shape
                        .0
                        .sheets
                        .clone()
                        .into_iter()
                        .enumerate()
                        .map(|(i, name)| {
                            let on = i == active;
                            view()
                                .label(name.clone())
                                .h(px(24.0))
                                .px(px(10.0))
                                .rounded(px(6.0))
                                .justify_center()
                                .bg(if on { theme::ACTIVE } else { AdaptiveColor::TRANSPARENT })
                                .child(text(name.clone()).text_size(px(12.0)).text_color(if on { theme::TEXT } else { theme::MUTED }))
                                .on_press(move || {
                                    commit_editing();
                                    send(Cmd::Sheet(i));
                                })
                                .into_element()
                        })
                        .collect::<Vec<_>>(),
                ),
            )
            .into_element()
    });
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
        if u.shape.with(|s| s.as_ref().is_some_and(|s| !s.0.editable)) {
            return "Opening for editing…".to_owned();
        }
        if st.dirty { "Edited".to_owned() } else { String::new() }
    };
    let zoom_btn = |label: &'static str, d: f32| {
        view()
            .label(if d > 0.0 { "Zoom in" } else { "Zoom out" })
            .size(px(22.0))
            .rounded(px(5.0))
            .items_center()
            .justify_center()
            .child(text(label).text_size(px(13.0)).text_color(theme::MUTED))
            .on_press(move || {
                let u = ui();
                let z = ((u.zoom.get_untracked() + d) * 10.0).round() / 10.0;
                u.zoom.set(z.clamp(0.5, 2.0));
            })
    };
    view()
        .flex_row()
        .items_center()
        .h(px(32.0))
        .px_2()
        .gap_2()
        .bg(theme::SUBTLE)
        .border_t(px(0.5))
        .border_t_color(theme::BORDER_S)
        .child(tabs)
        .child(text(status).number_of_lines(1).text_size(px(12.0)).text_color(theme::MUTED))
        .child(zoom_btn("−", -0.1))
        .child(text(move || format!("{}%", (u.zoom.get() * 100.0).round())).w(px(40.0)).text_center().text_size(px(12.0)).text_color(theme::MUTED))
        .child(zoom_btn("+", 0.1))
}

/// A bar over the grid: the file changed outside Rox while it held
/// edits, or closing asks what to do with them.
pub(crate) fn prompts() -> impl IntoElement {
    let button = |label: &'static str, primary: bool, on: fn()| {
        view()
            .label(label)
            .h(px(26.0))
            .px_3()
            .rounded(px(6.0))
            .justify_center()
            .bg(if primary { theme::ACCENT } else { theme::HOVER })
            .child(text(label).text_size(px(12.0)).text_color(if primary { theme::ON_FILL } else { theme::TEXT }))
            .on_press(move || on())
    };
    dynamic(move || {
        let st = store();
        let conflict = st.office.state.with(|s| s.conflict);
        let ask = st.office.ask_close.get();
        if !conflict && !ask {
            return view().into_element();
        }
        let name = st.viewing.get_untracked().map(|f| f.name).unwrap_or_default();
        let row = view().flex_row().items_center().gap_2().px_3().py_2().bg(theme::ACCENT_TINT).border_b(px(0.5)).border_b_color(theme::BORDER_S);
        if ask {
            row.child(text(format!("Save the changes to {name}?")).flex_1().text_size(px(13.0)).text_color(theme::TEXT))
                .child(button("Don't Save", false, || office::answer_close(Some(false))))
                .child(button("Cancel", false, || office::answer_close(None)))
                .child(button("Save", true, || office::answer_close(Some(true))))
                .into_element()
        } else {
            row.child(text(format!("{name} changed outside Rox while you were editing it.")).flex_1().text_size(px(13.0)).text_color(theme::TEXT))
                .child(button("Keep Mine", false, || office::keep_mine()))
                .child(button("Reload", true, || office::reload()))
                .into_element()
        }
    })
}

/// The spreadsheet view: prompts, the formula bar, the grid, the tabs.
pub(crate) fn sheet_view() -> impl IntoElement {
    let u = ui();
    view().flex_1().child(prompts()).child(formula_bar(u)).child(grid::grid(u)).child(status_bar(u))
}
