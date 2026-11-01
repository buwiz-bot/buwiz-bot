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
use buwiz_office_kit::host::{host, theme};

fn ui() -> &'static SheetUi {
    crate::FORMAT.ui()
}

fn send(cmd: Cmd) {
    crate::FORMAT.with_session(|s| s.send(cmd));
}

thread_local! {
    /// The cell editor, focused while the sheet shows.
    static FIELD: std::cell::Cell<Option<ElementRef>> = const { std::cell::Cell::new(None) };
}

/// The keyboard to the sheet (the cell editor).
pub fn focus() {
    if let Some(f) = FIELD.with(std::cell::Cell::get) {
        f.focus();
    }
}

/// Whether the sheet's keys apply: it shows and nothing is being edited.
pub fn navigating() -> bool {
    let u = ui();
    sheet_showing() && u.editing.with(Option::is_none) && u.chart.with(Option::is_none) && !host().ask_close().get() && !host().menu_open()
}

/// Whether a cell is being edited.
pub fn editing() -> bool {
    sheet_showing() && ui().editing.with(Option::is_some)
}

/// A spreadsheet shows natively: not a Word document, whose keys
/// (arrows, Shift-arrows, Backspace, ⌘Z…) the sheet's scope would take
/// first.
pub fn sheet_showing() -> bool {
    host().showing() && !host().is_word().get()
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
pub fn read_active() {
    let u = ui();
    let sheet = u.sheet();
    let cell = u.sel.get_untracked().active;
    send(Cmd::Read { sheet, cell });
}

/// An arrow, ⌘-arrow or page key; with Shift it extends.
pub fn nav(dir: Dir, step: Step, shift: bool) {
    let s = ui().sel.get_untracked();
    if let Some(next) = with_bounds(|b| s.step(dir, step, shift, b)) {
        set_sel(next);
    }
}

/// Tab, Shift-Tab, Enter, Shift-Enter: walk the range or move.
pub fn walk(dir: Dir) {
    let s = ui().sel.get_untracked();
    if let Some(next) = with_bounds(|b| s.walk(dir, b)) {
        set_sel(next);
    }
}

pub fn select_all() {
    commit_editing();
    let u = ui();
    u.sel.set(u.sel.get_untracked().all());
    focus();
}

/// A press on `cell` (Shift extends).
pub fn press(cell: (u32, u32), shift: bool) {
    let u = ui();
    super::chart::deselect();
    commit_editing();
    u.sel.set(u.sel.get_untracked().press(cell, shift));
    read_active();
    focus();
}

/// The link of `cell`, if the list in view has one there.
pub fn link_at((r, c): (u32, u32)) -> Option<betteroffice_xlsx::HyperlinkRegion> {
    ui().shown.with_untracked(|s| s.as_ref()?.list.hyperlinks.iter().find(|h| (h.top..=h.bottom).contains(&r) && (h.left..=h.right).contains(&c)).cloned())
}

thread_local! {
    /// The cell a link to another sheet goes to, once that sheet shows.
    static GO_TO: std::cell::Cell<Option<(usize, (u32, u32))>> = const { std::cell::Cell::new(None) };
}

/// A click on a link's cell, as the web editor takes it: a link out opens,
/// a link into the workbook (`'Sheet 2'!B3`) goes to its cell. False when
/// the cell has no link.
pub fn follow_link(cell: (u32, u32)) -> bool {
    let Some(h) = link_at(cell) else { return false };
    if let Some(url) = h.external_target {
        host().open_link(&url);
        return true;
    }
    let Some(loc) = h.location else { return true };
    let loc = loc.trim_start_matches('#');
    let (name, addr) = match loc.rsplit_once('!') {
        Some((n, a)) => (Some(n.trim_matches('\'').replace("''", "'")), a),
        None => (None, loc),
    };
    let addr = addr.split(':').next().unwrap_or_default().replace('$', "");
    let Ok(at) = betteroffice_xlsx::CellRef::parse_a1(&addr) else { return true };
    let to = (at.row.min(select::LAST_ROW), at.col.min(select::LAST_COL));
    let u = ui();
    let Some(shape) = u.shape.get_untracked() else { return true };
    let sheet = match name {
        Some(n) => match shape.0.sheets.iter().position(|s| s.eq_ignore_ascii_case(&n)) {
            Some(i) => i,
            None => return true,
        },
        None => shape.0.active,
    };
    if sheet == shape.0.active {
        set_sel(Sel::at(to));
    } else {
        GO_TO.with(|g| g.set(Some((sheet, to))));
        send(Cmd::Sheet(sheet));
    }
    true
}

/// Where a sheet just switched to opens: a link's cell, else A1.
pub fn opened_at(sheet: usize) -> Sel {
    match GO_TO.with(std::cell::Cell::take) {
        Some((s, cell)) if s == sheet => Sel::at(cell),
        _ => Sel::default(),
    }
}

/// Starts editing `cell`: with `typed` text replacing it, else its
/// content as the formula bar has it.
pub fn begin(cell: (u32, u32), typed: Option<String>) {
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
pub fn edit_active() {
    begin(ui().sel.get_untracked().active, None);
}

/// Sends the edit in progress (nothing when its text is unchanged).
pub fn commit_editing() {
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
pub fn commit_and(dir: Dir) {
    commit_editing();
    walk(dir);
    focus();
}

/// Escape while editing: the cell keeps what it had.
pub fn cancel() {
    ui().editing.set(None);
    focus();
}

/// Escape on the sheet: a range shrinks to its active cell, then the
/// viewer closes (asking when there are edits).
pub fn escape() {
    let u = ui();
    let s = u.sel.get_untracked();
    if !s.is_single() {
        u.sel.set(Sel::at(s.active));
        return;
    }
    if host().may_close() {
        host().close_viewer();
    }
}

pub fn copy(cut: bool) {
    let u = ui();
    let (sheet, range) = (u.sheet(), u.sel.get_untracked().range());
    send(Cmd::Copy { sheet, range });
    if cut {
        send(Cmd::Clear { sheet, range });
    }
}

/// ⌘V: tab-separated text at the range's top left.
pub fn paste() {
    let u = ui();
    let (sheet, (at, _)) = (u.sheet(), u.sel.get_untracked().range());
    let answer = clipboard_string();
    buwiz_office_kit::spawn_detached(async move {
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

pub fn clear() {
    let u = ui();
    send(Cmd::Clear { sheet: u.sheet(), range: u.sel.get_untracked().range() });
}

pub fn undo(redo: bool) {
    send(if redo { Cmd::Redo } else { Cmd::Undo });
}

/// The right-click menu over `cell`.
pub fn menu(cell: (u32, u32), x: f32, y: f32) {
    use buwiz_office_kit::host::MenuRow;
    let u = ui();
    super::chart::deselect();
    commit_editing();
    if !u.sel.get_untracked().contains(cell) {
        u.sel.set(Sel::at(cell));
        read_active();
    }
    let ((r0, c0), (r1, c1)) = u.sel.get_untracked().range();
    let rows = if r1 > r0 { format!("Delete {} Rows", r1 - r0 + 1) } else { "Delete Row".to_owned() };
    let cols = if c1 > c0 { format!("Delete {} Columns", c1 - c0 + 1) } else { "Delete Column".to_owned() };
    let items = vec![
        MenuRow::row("Cut", "copy", "sheet", "cut".into()),
        MenuRow::row("Copy", "copy", "sheet", "copy".into()),
        MenuRow::row("Paste", "copy", "sheet", "paste".into()),
        MenuRow::separator(),
        MenuRow::row("Insert Row Above", "plus", "sheet", "row-above".into()),
        MenuRow::row("Insert Row Below", "plus", "sheet", "row-below".into()),
        MenuRow::row("Insert Column Left", "plus", "sheet", "col-left".into()),
        MenuRow::row("Insert Column Right", "plus", "sheet", "col-right".into()),
        MenuRow::separator(),
        MenuRow::row(&rows, "trash", "sheet", "delete-rows".into()),
        MenuRow::row(&cols, "trash", "sheet", "delete-cols".into()),
        MenuRow::row("Clear Contents", "close", "sheet", "clear".into()),
    ];
    host().context_menu(x, y, items);
}

/// The right-click menu over a column's (`cols`) or row's header: what
/// changes the whole columns or rows, Hide and Unhide among it.
pub fn header_menu(cols: bool, i: u32, x: f32, y: f32) {
    use buwiz_office_kit::host::MenuRow;
    let u = ui();
    super::chart::deselect();
    commit_editing();
    let s = u.sel.get_untracked();
    let ((r0, c0), (r1, c1)) = s.range();
    let whole = if cols { r0 == 0 && r1 == select::LAST_ROW && (c0..=c1).contains(&i) } else { c0 == 0 && c1 == select::LAST_COL && (r0..=r1).contains(&i) };
    if !whole {
        u.sel.set(if cols { s.col(i, false) } else { s.row(i, false) });
        read_active();
    }
    let ((r0, c0), (r1, c1)) = u.sel.get_untracked().range();
    let items = if cols {
        let n = c1 - c0 + 1;
        vec![
            MenuRow::row("Insert Column Left", "plus", "sheet", "col-left".into()),
            MenuRow::row("Insert Column Right", "plus", "sheet", "col-right".into()),
            MenuRow::row(&if n > 1 { format!("Delete {n} Columns") } else { "Delete Column".to_owned() }, "trash", "sheet", "delete-cols".into()),
            MenuRow::row("Clear Contents", "close", "sheet", "clear".into()),
            MenuRow::separator(),
            MenuRow::row("Hide", "eyeOff", "sheet", "hide-cols".into()),
            MenuRow::row("Unhide", "eye", "sheet", "unhide-cols".into()),
        ]
    } else {
        let n = r1 - r0 + 1;
        vec![
            MenuRow::row("Insert Row Above", "plus", "sheet", "row-above".into()),
            MenuRow::row("Insert Row Below", "plus", "sheet", "row-below".into()),
            MenuRow::row(&if n > 1 { format!("Delete {n} Rows") } else { "Delete Row".to_owned() }, "trash", "sheet", "delete-rows".into()),
            MenuRow::row("Clear Contents", "close", "sheet", "clear".into()),
            MenuRow::separator(),
            MenuRow::row("Hide", "eyeOff", "sheet", "hide-rows".into()),
            MenuRow::row("Unhide", "eye", "sheet", "unhide-rows".into()),
        ]
    };
    host().context_menu(x, y, items);
}

/// A row of the sheet's menu.
pub fn action(what: &str) {
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
        "hide-cols" => super::resize::hide(true, true),
        "unhide-cols" => super::resize::hide(true, false),
        "hide-rows" => super::resize::hide(false, true),
        "unhide-rows" => super::resize::hide(false, false),
        _ => {}
    }
    focus();
}

/// The input over the active cell: invisible until something is typed.
pub fn cell_editor(u: &'static SheetUi) -> impl IntoElement {
    let field = element_ref();
    FIELD.with(|f| f.set(Some(field)));
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
        // a selected chart takes no typing (the web's grid has the keys)
        .readonly(move || u.chart.with(Option::is_some))
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
        .text_color(theme().text)
        .bg(theme().editor)
        .border(px(2.0))
        .border_color(theme().accent)
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
        .bg(theme().pane)
        .border_b(px(0.5))
        .border_b_color(theme().border_s)
        .child(
            text(move || select::address(&u.sel.get()))
                .w(px(96.0))
                .number_of_lines(1)
                .text_size(px(12.0))
                .text_color(theme().muted),
        )
        .child(text("ƒx").text_size(px(13.0)).text_color(theme().faint))
        .child(
            input()
                .label("Formula bar")
                .readonly(move || u.chart.with(Option::is_some))
                .flex_1()
                .h(px(24.0))
                .px_2()
                .rounded(px(5.0))
                .bg(theme().subtle)
                .text_size(px(13.0))
                .text_color(theme().text)
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
                                .bg(if on { theme().active } else { AdaptiveColor::TRANSPARENT })
                                .child(text(name.clone()).text_size(px(12.0)).text_color(if on { theme().text } else { theme().muted }))
                                .on_press(move || {
                                    super::chart::deselect();
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
        let st = host().state().get();
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
            .child(text(label).text_size(px(13.0)).text_color(theme().muted))
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
        .bg(theme().subtle)
        .border_t(px(0.5))
        .border_t_color(theme().border_s)
        .child(tabs)
        .child(text(status).number_of_lines(1).text_size(px(12.0)).text_color(theme().muted))
        .child(zoom_btn("−", -0.1))
        .child(text(move || format!("{}%", (u.zoom.get() * 100.0).round())).w(px(40.0)).text_center().text_size(px(12.0)).text_color(theme().muted))
        .child(zoom_btn("+", 0.1))
}

/// The spreadsheet view: prompts, the formula bar, the grid, the tabs.
pub fn sheet_view() -> impl IntoElement {
    let u = ui();
    view().flex_1().child(buwiz_office_kit::prompts::prompts()).child(formula_bar(u)).child(grid::grid(u)).child(status_bar(u))
}
