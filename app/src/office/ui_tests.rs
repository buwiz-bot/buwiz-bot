//! The office viewers driven as a person drives them, on a headless engine
//! (`nsh-testing`): the file viewer's layer (`files::layer`) and the
//! keymap's bindings, opening files from buttons as a reply's file cards
//! do, closing with the viewer's ✕, clicking, dragging, typing and keys in
//! the Word view, and the worker's answers coming back to the screen.
//!
//! The store is one per process (`live::store`), its signals in the graph
//! of the engine that made it: each test runs in a process of its own (the
//! test binary again, on just that test), so another test's engine never
//! holds it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use buwiz_office_kit::testing::{Headless, WAIT, num, read, run, until};
use nsh_core::element::Root;
use nsh_core::prelude::*;
use nsh_core::TouchPhase;
use nsh_engine::{App, EngineConfig};

use crate::live::store;
use crate::store::Attach;

/// Set in the child process that runs a test's body.
const CHILD: &str = "BUWIZ_OFFICE_UI_TEST";

/// The files the buttons open (`open 0`, `open 1`, …).
static FILES: OnceLock<Vec<Attach>> = OnceLock::new();
/// The menu row the `pick` button picks.
static PICK: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// The open menu's row [`PICK`] names, picked; the menu closes.
fn pick_row() {
    let label = PICK.lock().unwrap().clone();
    let item = store().ctx.get_untracked().and_then(|m| m.items.into_iter().find(|i| i.label == label));
    store().ctx.set(None);
    if let Some(i) = item {
        crate::ctx::act(&i);
    }
}

/// Whether this is the child that runs `name`'s body; the parent runs it
/// there and passes on its result.
fn in_child(name: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&format!("office::ui_tests::{name}"), "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        // nothing reaches a server: no network (the stub answers 404) and
        // an origin nothing listens on
        .env("BUWIZ_BACKEND", "http://127.0.0.1:1")
        // a reply's files: under the token's folder
        .env("BUWIZ_HOST_TOKEN_FILE", broker_dir(name).join("token"))
        .output()
        .expect("run the test's child");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
}

/// The broker's data folder a test's child takes (its files are in
/// `generated/files`).
fn broker_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rox-office-ui-broker-{name}"))
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("rox-office-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn attach(path: &Path, variant: &str) -> Attach {
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    Attach {
        kind: "file".to_owned(),
        path: path.to_string_lossy().into_owned(),
        ext: path.extension().unwrap().to_string_lossy().into_owned(),
        name,
        variant: variant.to_owned(),
        size: 1.0,
        ..Attach::default()
    }
}

/// A small workbook: numbers in A1:A5, formulas in C.
fn workbook() -> Vec<u8> {
    use betteroffice_xlsx::{Cell, CellRef, CellValue, Sheet, Workbook, WorkbookModel};
    let mut sheet = Sheet::new("Data");
    for r in 0..5u32 {
        sheet.set_cell(CellRef::new(r, 0), Cell { value: CellValue::Number { value: (r + 1) as f64 }, ..Default::default() });
    }
    let model = WorkbookModel { sheets: vec![sheet], ..WorkbookModel::default() };
    Workbook::from_model(model).unwrap().save().unwrap()
}

/// What the office UI holds now, as `key=value` words.
fn probe() -> String {
    let o = &store().office;
    let w = &o.word;
    let m = w.meta.get_untracked();
    let c = w.caret.get_untracked();
    let drawn = w.pages.borrow().first().and_then(|s| s.try_get()).flatten().map_or(0, |s| s.drawn.drawing.ops.len());
    let z = w.zoom.get_untracked();
    let left = m.pages.first().map_or(0.0, |p| super::docx::view::page_left(w.area.get_untracked().0, p.0 * z));
    let (pw, ph) = m.pages.first().copied().unwrap_or_default();
    let table = m.table.map_or("-".to_owned(), |(r, c)| format!("{r}x{c}"));
    let wm = m.watermark.as_ref().and_then(|v| v.get("text").or(v.get("kind"))).and_then(|v| v.as_str()).unwrap_or("-").replace(' ', "_");
    let pics = w.pages.borrow().iter().filter_map(|s| s.try_get().flatten()).map(|s| s.drawn.pictures.len()).sum::<usize>();
    let word = format!(" bidi={} table={table} wm={wm} pw={pw} ph={ph} mtop={} dialog={} pics={pics}", m.bidi, m.setup.top, (w.setup_open.get_untracked() || w.watermark_open.get_untracked()) as u8) + &fmt_probe();
    let (cp, cx, cy) = c.caret.map_or((-1, 0.0, 0.0), |(p, x, y, _)| (p as i64, x, y));
    let sheet = &o.sheet;
    format!(
        "probe viewing={} switch={} key={} pages={} drawn={drawn} size={} rev={} bold={} caret={cp} cx={cx} cy={cy} sel={} left={left} shape={} cells={} dirty={}",
        if store().viewing.get_untracked().is_some() { "open" } else { "-" },
        SWITCHED.with_borrow(|s| s.replace(' ', "_")),
        if o.key.get_untracked().is_empty() { "-" } else { "open" },
        m.pages.len(),
        m.fmt.size,
        m.rev,
        m.fmt.bold,
        c.sel.len(),
        sheet.shape.with_untracked(Option::is_some),
        sheet.shown.with_untracked(|s| s.is_some() as u8),
        o.state.get_untracked().dirty,
    ) + &word
        + &sheet_probe()
}

/// The formatting bar's part of the probe: what the worker said of the
/// selection, the mode, the zoom and the panels.
fn fmt_probe() -> String {
    let w = &store().office.word;
    let m = w.meta.get_untracked();
    let f = &m.fmt;
    let o = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".into()).replace(' ', "_");
    let cell = m.cell.as_ref().map_or("-".to_owned(), |c| format!("{}x{}{}", c.rows, c.cols, if c.multi { "+" } else { "" }));
    let fill = m.cell.as_ref().and_then(|c| c.fill.clone());
    let picture = w.caret.get_untracked().picture.is_some() as u8;
    let (picx, picy) = w.pages.borrow().first().and_then(|s| s.try_get().flatten()).and_then(|s| s.drawn.pictures.first().map(|p| (p.x, p.y))).unwrap_or_default();
    format!(
        " style={} family={} italic={} underline={} strike={} sup={} sub={} color={} hl={} align={} list={} ls={} indent={} link={} image={} cell={cell} fill={} mode={:?} review={} zoom={} picture={picture} picx={picx} picy={picy} linkdlg={}",
        f.style,
        f.family.replace(' ', "_"),
        f.italic,
        f.underline,
        f.strike,
        f.superscript,
        f.subscript,
        o(&f.color),
        o(&f.highlight),
        f.align,
        f.list.map_or("-".to_owned(), |(n, l)| format!("{n}.{l}")),
        f.line_spacing.unwrap_or(0.0),
        f.indent_left,
        o(&f.link.as_ref().map(|l| l.0.clone())),
        o(&f.image),
        o(&fill),
        m.mode,
        w.review.get_untracked(),
        w.zoom.get_untracked(),
        w.link_open.get_untracked() as u8,
    )
}

/// The spreadsheet's part of the probe: its revision and sheet, the
/// active cell, the chart selected and the first chart's frame (sheet
/// pixels), the nudge waiting, the note, the column widths and row
/// heights (pixels), the drawn selection's width, and whether the total
/// `$1,504,750.00` is drawn as a number or as `#####`.
fn sheet_probe() -> String {
    use crate::office::xlsx::grid;
    use nsh_core::canvas::CanvasOp;
    let u = &store().office.sheet;
    let shape = u.shape.get_untracked();
    let (srev, active, editable) = shape.as_ref().map_or((0, 0, false), |s| (s.0.rev, s.0.active, s.0.editable));
    let a = u.sel.get_untracked().active;
    let chart = u.chart.get_untracked().map_or("-".to_owned(), |c| c.id);
    let (charts, cx, cy, total) = u.shown.with_untracked(|s| {
        let Some(s) = s else { return (0, 0.0, 0.0, "-") };
        let (cx, cy) = s.list.charts.first().map_or((0.0, 0.0), |c| (c.rect.x / s.scale + s.vp.x, c.rect.y / s.scale + s.vp.y));
        let measure = nsh_core::element::text_measurer();
        let d = crate::office::draw::drawing(&s.list, &crate::office::draw::Palette::light(), &|f, t| measure(f, t).0);
        let texts: Vec<String> = d.ops.iter().filter_map(|op| if let CanvasOp::FillText { text, .. } = op { Some(text.to_string()) } else { None }).collect();
        let total = if texts.iter().any(|t| t == "$1,504,750.00") {
            "number"
        } else if texts.iter().any(|t| !t.is_empty() && t.chars().all(|c| c == '#')) {
            "hashes"
        } else {
            "-"
        };
        (s.list.charts.len(), cx, cy, total)
    });
    let nudge = u.nudge.get_untracked().map_or("-".to_owned(), |n| format!("{},{}", n.1, n.2));
    let note = u.note.get_untracked().replace(' ', "_");
    let (cols, rows) = shape.as_ref().map_or((String::new(), String::new()), |s| {
        let g = &s.0.geometry;
        let cols: Vec<String> = (0..10).map(|c| format!("{:.1}", g.col_x(c + 1) - g.col_x(c))).collect();
        let rows: Vec<String> = (0..6).map(|r| format!("{:.1}", g.row_y(r + 1) - g.row_y(r))).collect();
        (cols.join(","), rows.join(","))
    });
    let selw = grid::sel_range(u).map_or(0.0, |r| r.2 - r.0);
    let resizing = u.resize.get_untracked().is_some() as u8;
    format!(
        " srev={srev} active={active} editable={editable} act={},{} chart={chart} charts={charts} chartx={cx} charty={cy} total={total} nudge={nudge} note={note} cols={cols} rows={rows} selw={selw} drag={} resizing={resizing} editing={} menu={}",
        a.0,
        a.1,
        u.chart_drag.get_untracked().is_some() as u8,
        u.editing.get_untracked().is_some() as u8,
        store().ctx.get_untracked().is_some() as u8,
    )
}

thread_local! {
    /// The label of the switch the last `switch` press picked.
    static SWITCHED: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// The viewer's File actions menu (⌘K), and its view switch picked.
fn switch() {
    crate::files::viewer_actions();
    let item = store().ctx.get_untracked().and_then(|m| m.items.into_iter().find(|i| i.act == "office-view"));
    SWITCHED.with_borrow_mut(|s| *s = item.as_ref().map(|i| i.label.clone()).unwrap_or_default());
    match item {
        Some(i) => crate::ctx::act(&i),
        None => store().ctx.set(None),
    }
}

#[component]
fn Viewer() -> impl IntoElement {
    // the store lives as long as the app (as `App` makes it, first)
    Owner::detached().run(|| {
        let _ = store();
    });
    crate::office::track();
    let shown = signal(String::new());
    // the task runs while the root lives
    drop(set_interval(Duration::from_millis(20), move || shown.set(probe())));
    let files = FILES.get().cloned().unwrap_or_default();
    view()
        .size_full()
        .child(
            view()
                .flex_row()
                .h(px(24.0))
                .children(files.into_iter().enumerate().map(|(i, f)| view().label(format!("open {i}")).w(px(40.0)).h_full().on_press(move || store().open_file(&f)).into_element()))
                .child(view().label("save").w(px(40.0)).h_full().on_press(crate::office::save))
                .child(view().label("switch").w(px(40.0)).h_full().on_press(switch))
                .child(view().label("pick").w(px(40.0)).h_full().on_press(pick_row))
                .child(text(move || shown.get()).text_size(px(8.0))),
        )
        .child(view().flex_1().child(crate::files::layer()))
        .child(crate::keys::bindings())
}

fn start(files: Vec<Attach>) -> Headless {
    let _ = FILES.set(files);
    let mut config = EngineConfig::new(App::Rust(Root::new::<Viewer>(|| ())));
    config.viewport = Size::new(px(1200.0), px(900.0));
    // open dialogs come to the driver, which answers them
    config.environment.file_requests = true;
    run(config)
}

/// The row labelled `label` of the open menu, picked (on the engine's
/// thread, by the `pick` button).
trait Pick {
    fn pick(&mut self, label: &str);
}

impl Pick for Headless {
    fn pick(&mut self, label: &str) {
        *PICK.lock().unwrap() = label.to_owned();
        self.press("pick");
    }
}

/// A Word document shows: laid out, its first page drawn, the font size
/// read and its first page's view up.
fn word_shows(app: &mut Headless, what: &str) -> HashMap<String, String> {
    let p = until(app, what, |p| num(p, "pages") > 0.0 && num(p, "drawn") > 0.0 && num(p, "size") > 0.0);
    app.pump(Duration::from_millis(50));
    assert!(app.has("Page 1"), "{what}: no first page");
    p
}

fn sheet_shows(app: &mut Headless, what: &str) -> HashMap<String, String> {
    until(app, what, |p| p.get("shape").map(String::as_str) == Some("true") && num(p, "cells") > 0.0)
}

/// The viewer's ✕: `may_close`, then `viewing` off (the document closes
/// as the viewer leaves it).
fn close(app: &mut Headless) {
    app.press("Close");
    until(app, "the document closed", |p| p.get("key").map(String::as_str) == Some("-"));
}

#[test]
fn a_document_reopened_shows_again() {
    if !in_child("a_document_reopened_shows_again") {
        return;
    }
    let d = dir("reopen");
    let (a, b, x) = (d.join("a.docx"), d.join("b.docx"), d.join("c.xlsx"));
    std::fs::write(&a, super::docx::fixture::document(3)).unwrap();
    std::fs::write(&b, super::docx::fixture::paragraphs(&["Another file"])).unwrap();
    std::fs::write(&x, workbook()).unwrap();
    let mut app = start(vec![attach(&a, "docx"), attach(&b, "docx"), attach(&x, "spreadsheet")]);
    app.press("open 0");
    word_shows(&mut app, "the first open");
    close(&mut app);
    app.press("open 0");
    word_shows(&mut app, "the same file again");
    close(&mut app);
    app.press("open 1");
    word_shows(&mut app, "another file");
    close(&mut app);
    app.press("open 2");
    sheet_shows(&mut app, "a spreadsheet after a document");
    close(&mut app);
    app.press("open 2");
    sheet_shows(&mut app, "the spreadsheet again");
    close(&mut app);
    app.press("open 0");
    word_shows(&mut app, "the document after the spreadsheet");
}

#[test]
fn a_spreadsheet_reopened_shows_again() {
    if !in_child("a_spreadsheet_reopened_shows_again") {
        return;
    }
    let d = dir("reopen-sheet");
    let (x, y) = (d.join("a.xlsx"), d.join("b.xlsx"));
    std::fs::write(&x, workbook()).unwrap();
    std::fs::write(&y, workbook()).unwrap();
    let mut app = start(vec![attach(&x, "spreadsheet"), attach(&y, "spreadsheet")]);
    app.press("open 0");
    sheet_shows(&mut app, "the first open");
    close(&mut app);
    app.press("open 0");
    sheet_shows(&mut app, "the same file again");
    close(&mut app);
    app.press("open 1");
    sheet_shows(&mut app, "another file");
}

/// The body's paragraph texts of a saved file, and its document part.
fn saved(path: &Path) -> (Vec<String>, String) {
    let bytes = std::fs::read(path).unwrap();
    let d = betteroffice_docx::Document::open(&bytes).unwrap();
    let texts = d.paragraphs().iter().map(|p| betteroffice_docx::get_paragraph_text(p)).collect();
    let parts = ooxml_opc::unzip_parts(&bytes).unwrap();
    let xml = String::from_utf8_lossy(&parts.iter().find(|(n, _)| n == "word/document.xml").unwrap().1).into_owned();
    (texts, xml)
}

#[test]
fn the_word_view_takes_clicks_drags_keys_and_typing() {
    if !in_child("the_word_view_takes_clicks_drags_keys_and_typing") {
        return;
    }
    let d = dir("edit");
    let a = d.join("a.docx");
    std::fs::write(&a, super::docx::fixture::paragraphs(&["Hello world", "Second line"])).unwrap();
    let mut app = start(vec![attach(&a, "docx")]);
    app.press("open 0");
    let p = word_shows(&mut app, "the document");
    let (left, rev) = (num(&p, "left"), num(&p, "rev"));
    // a click right of the first line's end (the page's top margin is an
    // inch; pages start 24 points down) puts the caret at its end
    app.press_at("Document pages", left + 700.0, 24.0 + 104.0, false);
    let p = until(&mut app, "the click's caret", |p| num(p, "caret") == 0.0 && num(p, "cx") > 150.0 && num(p, "sel") == 0.0);
    let end_x = num(&p, "cx");
    // typing goes into the hidden field and on to the document, which lays
    // out and draws again
    app.type_text("Document", "!");
    let p = until(&mut app, "the typed text laid out", |p| p.get("dirty").map(String::as_str) == Some("true") && num(p, "rev") > rev);
    assert!(num(&p, "cx") > end_x, "the caret moved past the new character: {p:?}");
    // Shift-arrows select, ⌘B bolds the selection
    for _ in 0..6 {
        app.key("shift+left");
    }
    until(&mut app, "a selection", |p| num(p, "sel") > 0.0);
    app.key("mod+b");
    until(&mut app, "the selection bold", |p| p.get("bold").map(String::as_str) == Some("true"));
    // Return, typing and Backspace
    app.key("right");
    app.key("enter");
    app.type_text("Document", "Third");
    app.key("backspace");
    app.press("save");
    until(&mut app, "saved", |p| p.get("dirty").map(String::as_str) == Some("false"));
    let (texts, xml) = saved(&a);
    assert_eq!(texts, vec!["Hello world!", "Thir", "Second line"]);
    assert!(xml.contains("<w:b/>"), "{xml}");
    // a drag across the first line selects it
    app.press_at("Document pages", left + 100.0, 24.0 + 104.0, false);
    until(&mut app, "the caret at the line's start", |p| num(p, "sel") == 0.0 && num(p, "cx") < 150.0);
    app.drag_to("Document pages", left + 300.0, 24.0 + 104.0);
    until(&mut app, "the dragged selection", |p| num(p, "sel") > 0.0);
}

/// A document of two paragraphs open as `name`, its caret at the end of
/// the first line.
fn open_word(test: &str, name: &str) -> (Headless, PathBuf) {
    let d = dir(test);
    let a = d.join(name);
    std::fs::write(&a, super::docx::fixture::paragraphs(&["Hello world", "Second line"])).unwrap();
    let mut app = start(vec![attach(&a, "docx")]);
    app.press("open 0");
    let p = word_shows(&mut app, "the document");
    app.press_at("Document pages", num(&p, "left") + 700.0, 24.0 + 104.0, false);
    until(&mut app, "the caret at the line's end", |p| num(p, "caret") == 0.0 && num(p, "cx") > 150.0);
    (app, a)
}

fn is(p: &HashMap<String, String>, k: &str, v: &str) -> bool {
    p.get(k).map(String::as_str) == Some(v)
}

/// The menu bar's menu `name` opened: its rows' labels, in order.
fn menu_rows(app: &mut Headless, name: &str) -> Vec<String> {
    app.press(name);
    app.labels_in(&format!("{name} menu"))
}

#[test]
fn the_title_bar_names_the_document_and_each_menu_lists_the_webs_items() {
    if !in_child("the_title_bar_names_the_document_and_each_menu_lists_the_webs_items") {
        return;
    }
    let (mut app, _) = open_word("title", "Quarterly report.docx");
    // the name without `.docx`; nothing takes a new one, so it springs back
    assert!(app.has("Document icon"));
    assert_eq!(app.value("Document name"), "Quarterly report");
    app.change("Document name", "Other");
    app.pump(Duration::from_millis(100));
    assert_eq!(app.value("Document name"), "Quarterly report");
    // the first page: 24 down, at least 24 in, centred
    let page = app.frame("Page 1");
    let p = read(&app);
    assert_eq!(page.origin.y.0, 24.0);
    assert_eq!(page.origin.x.0, num(&p, "left"));
    assert!(num(&p, "left") >= 24.0);
    assert!(!app.texts().iter().any(|t| t.starts_with("Page 1 of")), "no status bar");
    // File: Save (⌘S), Print, Page setup; no Open
    assert_eq!(menu_rows(&mut app, "File"), ["Save", "Print", "Page setup"]);
    assert!(app.texts().contains(&"\u{2318}S"), "{:?}", app.texts());
    app.cancel("File menu");
    assert_eq!(menu_rows(&mut app, "Format"), ["Left-to-right text", "Right-to-left text"]);
    app.cancel("Format menu");
    assert_eq!(menu_rows(&mut app, "Insert"), ["Image", "Table", "Break", "Table of contents", "Watermark"]);
    app.hover("Break", true);
    assert_eq!(app.labels_in("Break submenu"), ["Page break", "Section break (next page)", "Section break (continuous)"]);
    // Table of contents is refused, as the web refuses it: dimmed, its
    // reason its tooltip (`chrome`'s tests), and a press does nothing
    assert!((app.style("Table of contents").opacity - 0.4).abs() < 1e-6);
    app.press("Table of contents");
    assert!(app.has("Insert menu"));
    app.cancel("Insert menu");
    assert_eq!(menu_rows(&mut app, "Help"), ["Report issue"]);
    app.cancel("Help menu");
}

#[test]
fn insert_puts_the_grids_table_and_each_break_at_the_caret() {
    if !in_child("insert_puts_the_grids_table_and_each_break_at_the_caret") {
        return;
    }
    let (mut app, _) = open_word("insert", "a.docx");
    let pages = num(&read(&app), "pages");
    app.press("Insert");
    app.hover("Table", true);
    app.hover("row 2 column 3", true);
    assert!(app.texts().contains(&"3 \u{d7} 2"), "{:?}", app.texts());
    app.press("row 2 column 3");
    until(&mut app, "two rows of three at the caret", |p| is(p, "table", "2x3"));
    assert!(!app.has("Insert menu"));
    app.key("mod+down");
    until(&mut app, "out of the table", |p| is(p, "table", "-"));
    let n = num(&read(&app), "pages");
    assert_eq!(n, pages);
    // in the last line, after "Second": each break where the caret is
    app.key("alt+left");
    for (row, want) in [("Section break (next page)", n + 1.0), ("Section break (continuous)", n + 1.0), ("Page break", n + 2.0)] {
        let rev = num(&read(&app), "rev");
        app.press("Insert");
        app.hover("Break", true);
        app.press(row);
        until(&mut app, row, |p| num(p, "rev") > rev && num(p, "pages") == want);
    }
}

#[test]
fn format_sets_the_paragraphs_direction() {
    if !in_child("format_sets_the_paragraphs_direction") {
        return;
    }
    let (mut app, _) = open_word("bidi", "a.docx");
    assert!(is(&read(&app), "bidi", "false"));
    app.press("Format");
    app.press("Right-to-left text");
    until(&mut app, "right to left", |p| is(p, "bidi", "true"));
    app.press("Format");
    app.press("Left-to-right text");
    until(&mut app, "left to right", |p| is(p, "bidi", "false"));
}

#[test]
fn page_setup_validates_and_lays_the_pages_out_at_its_size_and_margins() {
    if !in_child("page_setup_validates_and_lays_the_pages_out_at_its_size_and_margins") {
        return;
    }
    let (mut app, _) = open_word("setup", "a.docx");
    assert!(is(&read(&app), "pw", "816") && is(&read(&app), "mtop", "1440"));
    app.press("File");
    app.press("Page setup");
    assert!(app.has("Page Setup dialog"));
    for t in ["PAGE SIZE", "MARGINS", "Letter (8.5\" \u{d7} 11\")", "Portrait"] {
        assert!(app.texts().contains(&t), "{t} in {:?}", app.texts());
    }
    assert_eq!(app.value("Top"), "1");
    // a dialog has the keyboard: the page's keys stand back
    assert!(is(&read(&app), "dialog", "1"));
    app.press("Size");
    app.press("A4 (8.27\" \u{d7} 11.69\")");
    app.press("Orientation");
    app.press("Landscape");
    // what doesn't read as a number is not taken; what does is, in inches
    app.change("Top", "abc");
    app.change("Top", "0.5");
    app.press("Apply");
    let p = until(&mut app, "A4 landscape, a half-inch top", |p| is(p, "dialog", "0") && is(p, "mtop", "720") && (num(p, "pw") - 16838.0 / 15.0).abs() < 1.0);
    assert!((num(&p, "ph") - 11906.0 / 15.0).abs() < 1.0, "{p:?}");
    // opened again it shows what it set; Escape leaves it
    app.press("File");
    app.press("Page setup");
    assert!(app.texts().contains(&"A4 (8.27\" \u{d7} 11.69\")") && app.texts().contains(&"Landscape"), "{:?}", app.texts());
    assert_eq!(app.value("Top"), "0.5");
    app.cancel("Page Setup");
    until(&mut app, "closed", |p| is(p, "dialog", "0"));
}

#[test]
fn the_watermark_dialog_sets_a_text_watermark_and_takes_it_away() {
    if !in_child("the_watermark_dialog_sets_a_text_watermark_and_takes_it_away") {
        return;
    }
    let (mut app, _) = open_word("watermark", "a.docx");
    app.press("Insert");
    app.press("Watermark");
    assert!(app.has("Watermark dialog") && app.has("No watermark"));
    // a picture watermark needs its picture: Apply stays dimmed
    app.press("Picture watermark");
    assert!(app.has("Select Picture..."));
    assert!((app.style("Apply").opacity - 0.5).abs() < 1e-6);
    app.press("Apply");
    assert!(app.has("Watermark dialog"));
    app.press("Text watermark");
    assert_eq!(app.value("Text"), "CONFIDENTIAL");
    app.press("Preset");
    app.press("DRAFT");
    assert_eq!(app.value("Text"), "DRAFT");
    app.press("Apply");
    until(&mut app, "the DRAFT watermark", |p| is(p, "wm", "DRAFT") && is(p, "dialog", "0"));
    app.press("Insert");
    app.press("Watermark");
    app.press("No watermark");
    app.press("Apply");
    until(&mut app, "no watermark", |p| is(p, "wm", "-"));
}

#[test]
fn insert_image_puts_the_picked_picture_at_the_caret() {
    if !in_child("insert_image_puts_the_picked_picture_at_the_caret") {
        return;
    }
    let (mut app, a) = open_word("image", "a.docx");
    let png = a.with_file_name("dot.png");
    std::fs::write(&png, super::docx::fixture::png()).unwrap();
    assert!(is(&read(&app), "pics", "0"));
    app.press("Insert");
    app.press("Image");
    app.pick_file(&png);
    until(&mut app, "the picture on the page", |p| is(p, "pics", "1"));
}

/// The viewer's switch, from `state` (native or web) to the other.
fn flip(app: &mut Headless, label: &str, to_native: bool) {
    app.press("switch");
    let want = if to_native { "open" } else { "-" };
    let p = until(app, label, |p| p.get("key").map(String::as_str) == Some(want));
    assert_eq!(p.get("switch").map(String::as_str), Some(label.replace(' ', "_").as_str()), "{p:?}");
    assert_eq!(p.get("viewing").map(String::as_str), Some("open"), "the viewer stays on the file: {p:?}");
}

#[test]
fn the_viewer_switches_one_file_between_its_views() {
    if !in_child("the_viewer_switches_one_file_between_its_views") {
        return;
    }
    let files = broker_dir("the_viewer_switches_one_file_between_its_views").join("generated/files/files/m1");
    let _ = std::fs::remove_dir_all(&files);
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("a.docx"), super::docx::fixture::document(2)).unwrap();
    std::fs::write(files.join("c.xlsx"), workbook()).unwrap();
    let reply = |name: &str, variant: &str| Attach { path: format!("files/m1/{name}"), ..attach(Path::new(name), variant) };
    let mut app = start(vec![reply("a.docx", "docx"), reply("c.xlsx", "spreadsheet")]);
    app.press("open 0");
    word_shows(&mut app, "the document");
    flip(&mut app, "View in Web Editor", false);
    flip(&mut app, "View Natively", true);
    word_shows(&mut app, "the document back natively");
    // the web editor for this file only: another opens natively, as set
    flip(&mut app, "View in Web Editor", false);
    app.press("open 1");
    sheet_shows(&mut app, "the next file, natively");
    flip(&mut app, "View in Web Editor", false);
    flip(&mut app, "View Natively", true);
    sheet_shows(&mut app, "the spreadsheet back natively");
    app.press("open 0");
    word_shows(&mut app, "the document again, natively");
}

#[test]
fn each_change_of_the_word_field_inserts_only_what_is_new() {
    if !in_child("each_change_of_the_word_field_inserts_only_what_is_new") {
        return;
    }
    let d = dir("field");
    let a = d.join("a.docx");
    std::fs::write(&a, super::docx::fixture::paragraphs(&["Hello", "Second"])).unwrap();
    let mut app = start(vec![attach(&a, "docx")]);
    app.press("open 0");
    let p = word_shows(&mut app, "the document");
    let left = num(&p, "left");
    app.press_at("Document pages", left + 700.0, 24.0 + 104.0, false);
    until(&mut app, "the caret at the line's end", |p| num(p, "caret") == 0.0 && num(p, "cx") > 120.0);
    // the field never emptied between them: each change carries the last
    for v in ["a", "ab", "abc"] {
        app.change("Document", v);
    }
    // a replaced character (autocorrect, an input method's commit)
    app.change("Document", "abd");
    until(&mut app, "the typing laid out", |p| p.get("dirty").map(String::as_str) == Some("true"));
    app.press("save");
    until(&mut app, "saved", |p| p.get("dirty").map(String::as_str) == Some("false"));
    assert_eq!(saved(&a).0, vec!["Helloabd", "Second"]);
    // a key that moves the caret starts the field afresh: what it held is
    // not typed again
    app.key("left");
    app.change("Document", "x");
    until(&mut app, "the next typing", |p| p.get("dirty").map(String::as_str) == Some("true"));
    app.press("save");
    until(&mut app, "saved again", |p| p.get("dirty").map(String::as_str) == Some("false"));
    assert_eq!(saved(&a).0, vec!["Helloabxd", "Second"]);
}

/// The showcase-like workbook open, its editing engine in and its chart
/// drawn.
fn open_showcase(name: &str) -> (Headless, PathBuf) {
    let d = dir(name);
    let x = d.join("showcase.xlsx");
    std::fs::write(&x, super::xlsx::fixture::showcase()).unwrap();
    let mut app = start(vec![attach(&x, "spreadsheet")]);
    app.press("open 0");
    sheet_shows(&mut app, "the workbook");
    until(&mut app, "the editing engine and the chart", |p| p.get("editable").map(String::as_str) == Some("true") && num(p, "charts") == 1.0);
    (app, x)
}

fn word<'a>(p: &'a HashMap<String, String>, k: &str) -> &'a str {
    p.get(k).map_or("", String::as_str)
}

/// The column widths (pixels) the probe lists.
fn widths(p: &HashMap<String, String>, k: &str) -> Vec<f32> {
    word(p, k).split(',').filter_map(|v| v.parse().ok()).collect()
}

/// A saved workbook read again: its first sheet.
fn reopened(path: &Path) -> betteroffice_xlsx::Sheet {
    let wb = betteroffice_xlsx::Workbook::open(&std::fs::read(path).unwrap()).unwrap();
    wb.model().sheets[0].clone()
}

#[test]
fn a_chart_is_selected_dragged_nudged_and_deselected_as_on_the_web() {
    if !in_child("a_chart_is_selected_dragged_nudged_and_deselected_as_on_the_web") {
        return;
    }
    use crate::office::xlsx::chart::{MOVING, REFUSED};
    let (mut app, path) = open_showcase("chart");
    // the frame is H2:O17: K9 is under it
    let p = read(&app);
    let near = |a: f32, b: f32| (a - b).abs() < 0.5;
    let x0 = num(&p, "chartx");
    assert!(near(x0, 448.0) && near(num(&p, "charty"), 20.0), "{p:?}");
    let k9 = (10.5 * 64.0, 8.5 * 20.0);
    app.press_at("Cells", k9.0, k9.1, false);
    let p = until(&mut app, "the chart selected", |p| word(p, "chart") != "-");
    assert_eq!(word(&p, "act"), "0,0", "the cell under the chart is not selected: {p:?}");
    // a drag moves the outline, the release the chart: a column right, a row down
    // the chart's view, by what a screen reader reads of it
    let label = app.labels.iter().rev().find(|(id, l)| l.starts_with("Revenue by region") && app.live.contains(id)).map(|(_, l)| l.clone()).expect("the chart's view");
    let label = label.as_str();
    let rev = num(&p, "srev");
    app.touch_drag(label, k9, (64.0, 20.0), false);
    until(&mut app, "the outline dragged", |p| word(p, "drag") == "1");
    app.touch(label, TouchPhase::End, k9.0 + 64.0, k9.1 + 20.0);
    let p = until(&mut app, "the chart moved", |p| near(num(p, "chartx"), x0 + 64.0) && near(num(p, "charty"), 40.0));
    assert_eq!(num(&p, "srev"), rev + 1.0, "one move: {p:?}");
    assert_eq!(word(&p, "drag"), "0");
    // arrows nudge the outline at once, the chart in one move once they rest
    let rev = num(&p, "srev");
    for k in ["right", "right", "right", "shift+down"] {
        app.key(k);
    }
    // waited for, not read at once: each key pumps only 50 ms, and on a
    // loaded runner the last arrow's batch comes later (the probe then
    // still showed "3,0"); the send comes 250 ms after the last key
    let p = until(&mut app, "the outline nudged by all four arrows", |p| word(p, "nudge") == "3,10");
    assert_eq!(num(&p, "srev"), rev, "nothing sent yet: {p:?}");
    let p = until(&mut app, "the nudge sent", |p| near(num(p, "chartx"), x0 + 67.0) && near(num(p, "charty"), 50.0));
    assert_eq!(num(&p, "srev"), rev + 1.0, "the nudges as one move: {p:?}");
    assert_eq!(word(&p, "nudge"), "-");
    // Escape drops a nudge not sent and deselects
    app.key("right");
    app.key("escape");
    until(&mut app, "deselected", |p| word(p, "chart") == "-");
    app.pump(Duration::from_millis(500));
    let p = read(&app);
    assert!(near(num(&p, "chartx"), x0 + 67.0) && num(&p, "srev") == rev + 1.0, "the nudge dropped: {p:?}");
    // Delete does nothing to the chart or the cells under it
    let on = (12.0 * 64.0, 9.5 * 20.0);
    app.press_at("Cells", on.0, on.1, false);
    until(&mut app, "selected again", |p| word(p, "chart") != "-");
    app.key("delete");
    app.key("backspace");
    app.pump(Duration::from_millis(300));
    let p = read(&app);
    assert_eq!(num(&p, "srev"), rev + 1.0, "no edit: {p:?}");
    assert_ne!(word(&p, "chart"), "-");
    // the cell commands are refused with the web's texts
    app.key("mod+c");
    until(&mut app, "refused", |p| word(p, "note") == REFUSED.replace(' ', "_"));
    app.touch_drag(label, on, (30.0, 0.0), false);
    app.key("mod+v");
    until(&mut app, "refused mid-drag", |p| word(p, "note") == MOVING.replace(' ', "_"));
    // the drag lost: the chart stays
    app.touch(label, TouchPhase::Cancel, on.0 + 30.0, on.1);
    let p = until(&mut app, "the drag cancelled", |p| word(p, "drag") == "0");
    assert!(near(num(&p, "chartx"), x0 + 67.0), "{p:?}");
    // a double click does nothing, a right click opens no menu
    app.click("Cells", on.0, on.1, false, 2);
    app.right_click("Cells", on.0, on.1);
    let p = read(&app);
    assert_eq!((word(&p, "editing"), word(&p, "menu")), ("0", "0"), "{p:?}");
    // a press on a cell goes back to the cells
    app.press_at("Cells", 32.0, 50.0, false);
    let p = until(&mut app, "a cell", |p| word(p, "chart") == "-");
    assert_eq!(word(&p, "act"), "2,0");
    // saved: the anchor moved a column and a row, then 3 and 10 pixels
    app.press("save");
    until(&mut app, "saved", |p| p.get("dirty").map(String::as_str) == Some("false"));
    let sheet = reopened(&path);
    let from = sheet.charts[0].anchor.from_cell().unwrap();
    assert_eq!((from.row, from.col), (2, 8), "{from:?}");
    assert!(from.col_off > 0 && from.row_off > 0, "{from:?}");
}

#[test]
fn columns_and_rows_resize_autofit_hide_and_save_as_in_excel() {
    if !in_child("columns_and_rows_resize_autofit_hide_and_save_as_in_excel") {
        return;
    }
    let (mut app, path) = open_showcase("resize");
    let p = until(&mut app, "the total drawn", |p| word(p, "total") != "-");
    assert_eq!(word(&p, "total"), "hashes", "too wide for a default column: {p:?}");
    let w = widths(&p, "cols");
    let edge = |w: &[f32], c: usize| w[..=c].iter().sum::<f32>();
    // C's right border, dragged 40 points: 104 pixels, 14.14 characters
    app.hover_at("Column headers", edge(&w, 2), 10.0);
    assert!(app.has("Resize column"));
    app.touch_drag("Resize column", (edge(&w, 2), 10.0), (40.0, 0.0), false);
    let p = until(&mut app, "the drag", |p| word(p, "resizing") == "1");
    assert!(app.texts().contains(&"Width: 14.14 (104 pixels)"), "{:?}", app.texts());
    let rev = num(&p, "srev");
    app.touch("Resize column", TouchPhase::End, edge(&w, 2) + 40.0, 10.0);
    let p = until(&mut app, "C wider", |p| widths(p, "cols")[2] > 100.0);
    assert!((widths(&p, "cols")[2] - 104.0).abs() < 0.5, "{p:?}");
    assert_eq!(num(&p, "srev"), rev + 1.0);
    // B:D selected, D's border dragged: all three take the width
    let w = widths(&p, "cols");
    app.press_at("Column headers", edge(&w, 1) - 20.0, 10.0, false);
    app.press_at("Column headers", edge(&w, 3) - 20.0, 10.0, true);
    app.hover_at("Column headers", edge(&w, 3), 10.0);
    let to = 80.0 - w[3];
    app.touch_drag("Resize column", (edge(&w, 3), 10.0), (to, 0.0), true);
    let p = until(&mut app, "B:D sized", |p| (1..=3).all(|c| (widths(p, "cols")[c] - 80.0).abs() < 0.5));
    assert_eq!(num(&p, "srev"), rev + 2.0, "one step for three columns: {p:?}");
    // a double click on C's border fits B:D to their text: the total shows
    let w = widths(&p, "cols");
    app.hover_at("Column headers", edge(&w, 2), 10.0);
    app.click("Resize column", 4.0, 4.0, false, 2);
    let p = until(&mut app, "the total fits", |p| word(p, "total") == "number");
    assert_eq!(num(&p, "srev"), rev + 3.0, "{p:?}");
    let fitted = widths(&p, "cols")[2];
    // a row dragged taller, then fitted back to its automatic height
    let rows = widths(&p, "rows");
    let bottom = rows[..=2].iter().sum::<f32>();
    app.hover_at("Row headers", 10.0, bottom);
    assert!(app.has("Resize row"));
    app.touch_drag("Resize row", (10.0, bottom), (0.0, 20.0), true);
    until(&mut app, "row 3 taller", |p| (widths(p, "rows")[2] - 40.0).abs() < 0.5);
    app.hover_at("Row headers", 10.0, bottom + 20.0);
    app.click("Resize row", 4.0, 4.0, false, 2);
    until(&mut app, "row 3 automatic", |p| (widths(p, "rows")[2] - rows[2]).abs() < 0.5);
    // E hidden from its header's menu, then shown again
    let p = read(&app);
    let w = widths(&p, "cols");
    app.right_click("Column headers", edge(&w, 4) - 20.0, 10.0);
    app.pick("Hide");
    until(&mut app, "E hidden", |p| widths(p, "cols")[4] == 0.0);
    let p = read(&app);
    let w = widths(&p, "cols");
    app.press_at("Column headers", edge(&w, 3) - 20.0, 10.0, false);
    app.press_at("Column headers", edge(&w, 5) - 20.0, 10.0, true);
    app.right_click("Column headers", edge(&w, 3) - 20.0, 10.0);
    app.pick("Unhide");
    until(&mut app, "E shown", |p| widths(p, "cols")[4] > 60.0);
    // saved and read again: the fitted width
    app.press("save");
    until(&mut app, "saved", |p| p.get("dirty").map(String::as_str) == Some("false"));
    let sheet = reopened(&path);
    let chars = *sheet.col_widths.get(&2).unwrap();
    assert!((xlsx_render::geometry::col_chars_to_px(chars) - fitted).abs() < 0.5, "{chars} for {fitted}");
    assert!(!sheet.col_hidden(4));
}

#[test]
fn merged_cells_select_whole_and_a_link_opens_on_a_click() {
    if !in_child("merged_cells_select_whole_and_a_link_opens_on_a_click") {
        return;
    }
    let (mut app, _) = open_showcase("merged");
    // the title is merged over A1:E1: a press on B1 selects all of it
    let p = read(&app);
    let w = widths(&p, "cols");
    app.press_at("Cells", w[0] + 10.0, 10.0, false);
    let p = until(&mut app, "B1", |p| word(p, "act") == "0,1");
    assert!((num(&p, "selw") - w[..5].iter().sum::<f32>()).abs() < 0.5, "{p:?}");
    // A11 links to Notes!B3: a plain click goes there
    app.press_at("Cells", 10.0, 10.5 * 20.0, false);
    let p = until(&mut app, "the link followed", |p| word(p, "active") == "1");
    assert_eq!(word(&p, "act"), "2,1", "{p:?}");
}

// ---- the formatting bar ----

/// [`open_word`] in a window `w` wide.
fn open_word_at(test: &str, name: &str, w: f32) -> (Headless, PathBuf) {
    let d = dir(test);
    let a = d.join(name);
    std::fs::write(&a, super::docx::fixture::paragraphs(&["Hello world", "Second line"])).unwrap();
    let mut app = start(vec![attach(&a, "docx")]);
    app.resize(w, 900.0);
    app.press("open 0");
    let p = word_shows(&mut app, "the document");
    app.press_at("Document pages", num(&p, "left") + 700.0, 24.0 + 104.0, false);
    until(&mut app, "the caret at the line's end", |p| num(p, "caret") == 0.0 && num(p, "cx") > 150.0);
    (app, a)
}

/// A lit toggle's fill (`--doc-toggle-active-bg`), light.
fn lit() -> nsh_core::Color {
    nsh_core::Color::from(super::ui::palette::TOGGLE_ACTIVE_BG.light)
}

/// `want` in `have`, in order (other labels between).
fn in_order(have: &[String], want: &[&str]) {
    let mut at = 0;
    for w in want {
        match have[at..].iter().position(|h| h == w) {
            Some(i) => at += i + 1,
            None => panic!("{w:?} after {:?} in {have:?}", have.get(at.saturating_sub(1))),
        }
    }
}

#[test]
fn the_formatting_bar_lists_the_webs_controls_in_order() {
    if !in_child("the_formatting_bar_lists_the_webs_controls_in_order") {
        return;
    }
    let (app, _) = open_word_at("bar-order", "a.docx", 1700.0);
    let labels = app.labels_by_x("Formatting toolbar");
    let groups = ["History", "Zoom", "Styles", "Font", "Text formatting", "Script", "Alignment", "List formatting", "Image", "Table"];
    let shown: Vec<&String> = labels.iter().filter(|l| groups.contains(&l.as_str())).collect();
    assert_eq!(shown, groups, "every group's place, the Image and Table ones kept for their turn");
    in_order(
        &labels,
        &[
            "History", "Undo", "Redo", "Zoom", "Zoom out", "Zoom level", "Zoom in", "Styles", "Select paragraph style", "Font", "Select font family", "Decrease font size", "Select font size", "Increase font size",
            "Text formatting", "Bold", "Italic", "Underline", "Strikethrough", "Font Color", "Text Highlight Color", "Insert link", "Script", "Superscript", "Subscript", "Alignment", "Align Left (Ctrl+L)",
            "List formatting", "Bullet List", "Numbered List", "Decrease Indent", "Increase Indent", "Line spacing", "Image", "Table", "Clear formatting", "Separator", "Comments & Changes", "Separator", "Editing",
        ],
    );
    // the contextual groups have no controls out of their context
    for c in ["Text wrapping", "Transform", "Image properties", "Borders", "More table options"] {
        assert!(!app.has(c), "{c} out of its context");
    }
    assert!(!app.has("More actions"), "all fit");
    // no glyph buttons of the old bar
    assert!(!app.has("Find") && !app.has("Tracked changes and comments"));
    // undo is off with nothing to undo; the editing mode's label shows on a wide window
    assert!((app.style("Undo").opacity - 0.3).abs() < 1e-6);
    assert!(app.texts().contains(&"Editing"));
}

#[test]
fn the_bars_controls_format_the_selection_and_light() {
    if !in_child("the_bars_controls_format_the_selection_and_light") {
        return;
    }
    let (mut app, a) = open_word_at("bar-format", "a.docx", 1700.0);
    app.key("alt+shift+left");
    until(&mut app, "\"world\" selected", |p| num(p, "sel") > 0.0);
    for (button, key) in [("Bold", "bold"), ("Italic", "italic"), ("Underline", "underline"), ("Strikethrough", "strike"), ("Superscript", "sup")] {
        app.press(button);
        until(&mut app, button, |p| is(p, key, "true"));
        // lit: the toggle's active fill
        assert_eq!(app.style(button).background_color, lit(), "{button} lit");
    }
    app.press("Subscript");
    until(&mut app, "subscript, not superscript", |p| is(p, "sub", "true") && is(p, "sup", "false"));
    app.press("Font Color choices");
    app.press("Red");
    until(&mut app, "red", |p| is(p, "color", "FF0000"));
    app.press("Text Highlight Color");
    until(&mut app, "yellow", |p| is(p, "hl", "FFFF00"));
    app.press("Select font family");
    app.press("Georgia");
    until(&mut app, "Georgia", |p| is(p, "family", "Georgia"));
    app.press("Increase font size");
    until(&mut app, "12", |p| num(p, "size") == 12.0);
    app.press("Select font size");
    app.press("24");
    until(&mut app, "24", |p| num(p, "size") == 24.0);
    app.press("Clear formatting");
    until(&mut app, "cleared", |p| is(p, "bold", "false") && is(p, "italic", "false") && is(p, "color", "-") && num(p, "size") == 11.0);
    // the paragraph: a style, each alignment, lists, line spacing
    app.press("Select paragraph style");
    app.press("Heading 1");
    until(&mut app, "Heading 1", |p| is(p, "style", "Heading1"));
    for (row, align) in [("Center", "center"), ("Align Right", "right"), ("Justify", "both"), ("Align Left", "left")] {
        let trigger = app.labels_in("Alignment").into_iter().find(|l| l.contains("(Ctrl+")).unwrap();
        app.press(&trigger);
        app.press(row);
        until(&mut app, row, |p| is(p, "align", align));
    }
    app.press("Bullet List");
    until(&mut app, "bullets", |p| is(p, "list", "1.0"));
    assert_eq!(app.style("Bullet List").background_color, lit(), "bullets lit");
    app.press("Increase Indent");
    until(&mut app, "a level in", |p| is(p, "list", "1.1"));
    app.press("Decrease Indent");
    until(&mut app, "a level out", |p| is(p, "list", "1.0"));
    app.press("Numbered List");
    until(&mut app, "numbers", |p| is(p, "list", "2.0"));
    app.press("Line spacing");
    app.press("1.5");
    until(&mut app, "1.5 lines", |p| num(p, "ls") == 360.0);
    // the zoom presets, and its steps
    app.press("Zoom level");
    app.press("150%");
    until(&mut app, "150%", |p| num(p, "zoom") == 1.5);
    app.press("Zoom in");
    until(&mut app, "200%", |p| num(p, "zoom") == 2.0);
    app.press("Zoom out");
    until(&mut app, "back to 150%", |p| num(p, "zoom") == 1.5);
    app.press("save");
    until(&mut app, "saved", |p| is(p, "dirty", "false"));
    let (_, xml) = saved(&a);
    for want in [r#"<w:pStyle w:val="Heading1"/>"#, r#"<w:numId w:val="2"/>"#, r#"w:line="360""#, r#"<w:jc w:val="left"/>"#] {
        assert!(xml.contains(want), "{want} in {xml}");
    }
}

#[test]
fn the_webs_chords_align_script_and_link() {
    if !in_child("the_webs_chords_align_script_and_link") {
        return;
    }
    let (mut app, _) = open_word_at("bar-keys", "a.docx", 1700.0);
    for (k, align) in [("mod+e", "center"), ("mod+r", "right"), ("mod+j", "both"), ("mod+l", "left")] {
        app.key(k);
        until(&mut app, k, |p| is(p, "align", align));
    }
    app.key("alt+shift+left");
    app.key("mod+=");
    until(&mut app, "subscript", |p| is(p, "sub", "true"));
    app.key("mod+shift+=");
    until(&mut app, "superscript", |p| is(p, "sup", "true") && is(p, "sub", "false"));
    app.key("mod+b");
    until(&mut app, "bold still", |p| is(p, "bold", "true"));
    app.key("mod+k");
    until(&mut app, "the link dialog", |p| is(p, "linkdlg", "1"));
    assert!(app.has("Insert Hyperlink dialog"));
}

#[test]
fn a_link_inserts_and_edits_through_its_dialog() {
    if !in_child("a_link_inserts_and_edits_through_its_dialog") {
        return;
    }
    let (mut app, a) = open_word_at("bar-link", "a.docx", 1700.0);
    app.key("alt+shift+left");
    until(&mut app, "\"world\" selected", |p| num(p, "sel") > 0.0);
    app.press("Insert link");
    assert!(app.has("Insert Hyperlink dialog") && app.has("Close dialog"));
    for t in ["URL", "Display Text", "Tooltip (optional)", "Enter a web address, email (mailto:), or phone (tel:)", "Leave empty to use the selected text", "Cancel", "Insert"] {
        assert!(app.texts().contains(&t), "{t} in {:?}", app.texts());
    }
    assert_eq!(app.value("Display Text"), "world");
    assert!(!app.has("Remove Link") || (app.style("Remove Link").opacity).abs() < 1e-6, "no Remove Link for a new link");
    app.change("URL", "example.com");
    app.press("Insert");
    until(&mut app, "the link", |p| is(p, "link", "https://example.com") && is(p, "linkdlg", "0"));
    // the caret in the link: Edit Hyperlink, with the link's address
    app.key("left");
    app.key("mod+k");
    assert!(app.has("Edit Hyperlink dialog"));
    assert_eq!(app.value("URL"), "https://example.com");
    assert!(app.texts().contains(&"Update") && app.texts().contains(&"Remove Link"));
    // an address that doesn't read as one is not taken
    app.change("URL", "exa mple");
    app.press("Update");
    assert!(app.texts().contains(&"Please enter a valid URL"), "{:?}", app.texts());
    app.change("URL", "https://example.org");
    app.press("Update");
    until(&mut app, "the link edited", |p| is(p, "link", "https://example.org") && is(p, "linkdlg", "0"));
    app.press("save");
    until(&mut app, "saved", |p| is(p, "dirty", "false"));
    let bytes = std::fs::read(&a).unwrap();
    let parts = ooxml_opc::unzip_parts(&bytes).unwrap();
    let rels = String::from_utf8_lossy(&parts.iter().find(|(n, _)| n == "word/_rels/document.xml.rels").unwrap().1).into_owned();
    assert!(rels.contains("https://example.org"), "{rels}");
    // Remove Link takes it off
    app.key("mod+k");
    app.press("Remove Link");
    until(&mut app, "no link", |p| is(p, "link", "-"));
}

#[test]
fn the_table_group_shows_in_a_table_and_its_commands_work() {
    if !in_child("the_table_group_shows_in_a_table_and_its_commands_work") {
        return;
    }
    let (mut app, _) = open_word_at("bar-table", "a.docx", 1700.0);
    assert!(!app.has("More table options"));
    app.press("Insert");
    app.hover_in("Insert menu", "Table");
    app.hover("row 2 column 2", true);
    app.press("row 2 column 2");
    until(&mut app, "in the new table", |p| is(p, "cell", "2x2"));
    for c in ["Borders", "Border Color", "Border width", "Cell Fill Color", "More table options"] {
        assert!(app.has(c), "{c} in the table group");
    }
    app.press("More table options");
    let rows = app.labels_in("More table options menu");
    in_order(
        &rows,
        &[
            "Insert row above", "Insert row below", "Insert column left", "Insert column right", "Merge cells", "Split cell", "Select entire table", "Delete row", "Delete column", "Delete table", "Top", "Middle", "Bottom",
            "Align table left", "Align table center", "Align table right", "Toggle header row", "Distribute columns evenly", "Auto-fit to contents", "Toggle no-wrap", "Table properties...",
        ],
    );
    app.press("Insert row below");
    until(&mut app, "a row more", |p| is(p, "cell", "3x2"));
    app.press("More table options");
    app.press("Insert column right");
    until(&mut app, "a column more", |p| is(p, "cell", "3x3"));
    // the refused rows: greyed, and a press does nothing
    app.press("More table options");
    app.press("Toggle header row");
    assert!(app.has("More table options menu"), "the refused row keeps the menu");
    app.cancel("More table options menu");
    app.press("Cell Fill Color");
    until(&mut app, "the cell filled", |p| is(p, "fill", "FFFF00"));
    let rev = num(&read(&app), "rev");
    app.press("Borders");
    app.press("All borders");
    until(&mut app, "borders", |p| num(p, "rev") > rev);
    let rev = num(&read(&app), "rev");
    app.press("Border width");
    app.press("1.5 pt");
    until(&mut app, "a wider border", |p| num(p, "rev") > rev);
    app.press("More table options");
    app.press("Select entire table");
    until(&mut app, "the whole table", |p| is(p, "cell", "3x3+"));
    let rev = num(&read(&app), "rev");
    app.press("More table options");
    app.press("Merge cells");
    // one cell over the grid: the selection is the caret's cell again
    until(&mut app, "merged", |p| num(p, "rev") > rev && is(p, "cell", "3x3"));
    app.press("More table options");
    app.press("Delete table");
    until(&mut app, "no table", |p| is(p, "cell", "-"));
    assert!(!app.has("More table options"));
}

#[test]
fn the_image_group_shows_on_a_selected_picture() {
    if !in_child("the_image_group_shows_on_a_selected_picture") {
        return;
    }
    let (mut app, a) = open_word_at("bar-image", "a.docx", 1700.0);
    let png = a.with_file_name("dot.png");
    std::fs::write(&png, super::docx::fixture::png()).unwrap();
    app.press("Insert");
    app.press("Image");
    app.pick_file(&png);
    let p = until(&mut app, "the picture on the page", |p| is(p, "pics", "1"));
    assert!(!app.has("Text wrapping"));
    // a click on it selects it, outlined
    app.press_at("Document pages", num(&p, "left") + num(&p, "picx") + 1.0, 24.0 + num(&p, "picy") + 1.0, false);
    until(&mut app, "the picture selected", |p| is(p, "image", "inline") && is(p, "picture", "1"));
    for c in ["Text wrapping", "Transform", "Image properties"] {
        assert!(app.has(c), "{c} in the image group");
    }
    app.press("Text wrapping");
    app.press("Square Left");
    until(&mut app, "wrapped", |p| is(p, "image", "wrapRight"));
    let rev = num(&read(&app), "rev");
    app.press("Transform");
    app.press("Rotate clockwise");
    until(&mut app, "turned", |p| num(p, "rev") > rev);
    // a caret in the text: the group goes
    app.key("mod+down");
    until(&mut app, "no picture", |p| is(p, "image", "-"));
    assert!(!app.has("Text wrapping"));
}

#[test]
fn suggesting_tracks_edits_and_viewing_blocks_typing() {
    if !in_child("suggesting_tracks_edits_and_viewing_blocks_typing") {
        return;
    }
    let (mut app, a) = open_word_at("bar-modes", "a.docx", 1700.0);
    app.press("Editing");
    for row in ["Editing", "Suggesting", "Viewing", "Edit document directly", "Edits become suggestions", "Read-only, no edits"] {
        assert!(app.texts().contains(&row), "{row} in {:?}", app.texts());
    }
    app.press("Suggesting");
    until(&mut app, "suggesting", |p| is(p, "mode", "Suggesting"));
    app.type_text("Document", "!");
    // a word of what was there deleted (deleting one's own suggestion takes it back)
    app.key("mod+up");
    app.key("alt+shift+right");
    app.key("backspace");
    app.press("save");
    until(&mut app, "saved", |p| is(p, "dirty", "false"));
    let (_, xml) = saved(&a);
    assert!(xml.contains("<w:ins ") && xml.contains("<w:del "), "{xml}");
    app.press("Suggesting");
    app.press("Viewing");
    let p = until(&mut app, "viewing", |p| is(p, "mode", "Viewing"));
    // the bar's commands are off, and typing goes nowhere
    assert!((app.style("Bold").opacity - 0.3).abs() < 1e-6);
    app.type_text("Document", "x");
    app.key("enter");
    app.pump(Duration::from_millis(200));
    let after = read(&app);
    assert_eq!((num(&after, "rev"), after.get("dirty").cloned()), (num(&p, "rev"), Some("false".to_owned())), "nothing typed in");
}

#[test]
fn the_bar_overflows_into_more_actions_at_a_narrow_width() {
    if !in_child("the_bar_overflows_into_more_actions_at_a_narrow_width") {
        return;
    }
    let (mut app, _) = open_word_at("bar-overflow", "a.docx", 1700.0);
    assert!(!app.has("More actions"));
    app.resize(760.0, 900.0);
    let end = Instant::now() + WAIT;
    while !app.has("More actions") {
        assert!(Instant::now() < end, "no More menu at 760");
        app.pump(Duration::from_millis(50));
    }
    // the first parts stay; the last went, whole, into the menu
    assert!(app.has("Undo") && app.has("Select paragraph style"));
    assert!(!app.has("Comments & Changes") && !app.has("Bullet List"));
    app.press("More actions");
    let rows = app.labels_in("More actions menu");
    in_order(&rows, &["Bullet List", "Numbered List", "Decrease Indent", "Increase Indent", "Line spacing", "Clear formatting", "Comments & Changes", "Editing mode"]);
    assert!(app.texts().contains(&"List formatting"), "the group's heading");
    assert!(!rows.iter().any(|r| r == "Undo"), "{rows:?}");
    app.press("Bullet List");
    until(&mut app, "bullets from the menu", |p| is(p, "list", "1.0"));
    // wide again: everything back on the bar
    app.resize(1700.0, 900.0);
    let end = Instant::now() + WAIT;
    while app.has("More actions") {
        assert!(Instant::now() < end, "the More menu stays at 1700");
        app.pump(Duration::from_millis(50));
    }
    assert!(app.has("Comments & Changes"));
}
