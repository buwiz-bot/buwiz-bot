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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, mpsc};
use std::time::{Duration, Instant};

use nsh_core::element::Root;
use nsh_core::keymap::{KeyMatch, Keymap, Keystroke};
use nsh_core::platform::Os;
use nsh_core::prelude::*;
use nsh_core::{ApproxTextEngine, EventKind, EventPayload, MountBatch, MountItem, NodeId, PayloadValue, ROOT_ID};
use nsh_engine::{App, Engine, EngineConfig, MountSink};

use crate::live::store;
use crate::store::Attach;

/// Set in the child process that runs a test's body.
const CHILD: &str = "ROX_OFFICE_UI_TEST";
const WAIT: Duration = Duration::from_secs(20);

/// The files the buttons open (`open 0`, `open 1`, …).
static FILES: OnceLock<Vec<Attach>> = OnceLock::new();

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
        .env("ROX_BACKEND", "http://127.0.0.1:1")
        .output()
        .expect("run the test's child");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed in its child:\n{log}");
    false
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
    let left = m.pages.first().map_or(0.0, |p| ((w.area.get_untracked().0 - p.0 * z) / 2.0).max(16.0));
    let (cp, cx, cy) = c.caret.map_or((-1, 0.0, 0.0), |(p, x, y, _)| (p as i64, x, y));
    let sheet = &o.sheet;
    format!(
        "probe key={} pages={} drawn={drawn} size={} rev={} bold={} caret={cp} cx={cx} cy={cy} sel={} left={left} shape={} cells={} dirty={}",
        if o.key.get_untracked().is_empty() { "-" } else { "open" },
        m.pages.len(),
        m.size,
        m.rev,
        m.bold,
        c.sel.len(),
        sheet.shape.with_untracked(Option::is_some),
        sheet.shown.with_untracked(|s| s.is_some() as u8),
        o.state.get_untracked().dirty,
    )
}

#[component]
fn Viewer() -> impl IntoElement {
    // the store lives as long as the app (as `App` makes it, first)
    Owner::detached().run(|| {
        let _ = store();
    });
    crate::office::track();
    let shown = signal(String::new());
    let _ = set_interval(Duration::from_millis(20), move || shown.set(probe()));
    let files = FILES.get().cloned().unwrap_or_default();
    view()
        .size_full()
        .child(
            view()
                .flex_row()
                .h(px(24.0))
                .children(files.into_iter().enumerate().map(|(i, f)| view().label(format!("open {i}")).w(px(40.0)).h_full().on_press(move || store().open_file(&f)).into_element()))
                .child(view().label("save").w(px(40.0)).h_full().on_press(crate::office::save))
                .child(text(move || shown.get()).text_size(px(8.0))),
        )
        .child(view().flex_1().child(crate::files::layer()))
        .child(crate::keys::bindings())
}

struct Sink(mpsc::Sender<MountBatch>);

impl MountSink for Sink {
    fn submit(&self, batch: MountBatch) {
        let _ = self.0.send(batch);
    }
}

/// The engine and what its batches mounted: the views alive, their
/// labels and texts, the inputs' values and the keymap.
struct Headless {
    engine: Engine,
    rx: mpsc::Receiver<MountBatch>,
    live: HashSet<NodeId>,
    labels: Vec<(NodeId, String)>,
    texts: HashMap<NodeId, String>,
    inputs: HashMap<NodeId, (String, u32)>,
    keymap: Option<std::sync::Arc<Keymap>>,
}

fn start(files: Vec<Attach>) -> Headless {
    let _ = FILES.set(files);
    let (tx, rx) = mpsc::channel();
    let mut config = EngineConfig::new(App::Rust(Root::new::<Viewer>(|| ())));
    config.viewport = Size::new(px(1200.0), px(900.0));
    let engine = Engine::start(config, || ApproxTextEngine, Sink(tx)).expect("engine starts");
    let mut app = Headless { engine, rx, live: HashSet::new(), labels: Vec::new(), texts: HashMap::new(), inputs: HashMap::new(), keymap: None };
    until(&mut app, "the first render", |_| true);
    app
}

impl Headless {
    fn apply(&mut self, batch: &MountBatch) {
        for item in &batch.items {
            match item {
                MountItem::Create { id, .. } => {
                    self.live.insert(*id);
                }
                MountItem::Delete { id } => {
                    self.live.remove(id);
                    self.texts.remove(id);
                }
                MountItem::AccessibilityLabel { id, label } => {
                    if let Some(l) = label {
                        self.labels.push((*id, l.to_string()));
                    }
                }
                MountItem::ViewProps { id, props } => {
                    if let Some(l) = props.accessibility.label.as_deref() {
                        self.labels.push((*id, l.to_owned()));
                    }
                }
                MountItem::Text { id, text, .. } => {
                    self.texts.insert(*id, text.text.clone());
                }
                MountItem::Input { id, .. } => {
                    self.inputs.entry(*id).or_default();
                }
                MountItem::InputValue { id, text, .. } => {
                    self.inputs.entry(*id).or_default().0 = text.clone();
                }
                MountItem::Keymap(k) => self.keymap = Some(k.clone()),
                _ => {}
            }
        }
    }

    /// Applies what comes for `time`.
    fn pump(&mut self, time: Duration) {
        let end = Instant::now() + time;
        while let Ok(b) = self.rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            self.apply(&b);
        }
    }

    fn texts(&self) -> Vec<&str> {
        self.texts.iter().filter(|(id, _)| self.live.contains(id)).map(|(_, t)| t.as_str()).collect()
    }

    /// The newest live view labelled `label`.
    fn view(&self, label: &str) -> NodeId {
        self.labels.iter().rev().find(|(id, l)| l == label && self.live.contains(id)).map(|(id, _)| *id).unwrap_or_else(|| panic!("no view labelled {label:?}"))
    }

    fn send(&self, id: NodeId, kind: EventKind, payload: EventPayload) {
        self.engine.control().dispatch_event(id, kind, payload);
    }

    /// A click at `(x, y)` in the view labelled `label` (`shift` held).
    fn press_at(&mut self, label: &str, x: f32, y: f32, shift: bool) {
        let id = self.view(label);
        let e = PressEvent { position: point(px(x), px(y)), page_position: point(px(x), px(y)), modifiers: nsh_core::keymap::Modifiers { shift, ..Default::default() }, clicks: 1 };
        for kind in [EventKind::PressDown, EventKind::PressUp, EventKind::Press] {
            self.send(id, kind, EventPayload::Press(e.clone()));
        }
        self.pump(Duration::from_millis(50));
    }

    fn press(&mut self, label: &str) {
        self.press_at(label, 2.0, 2.0, false);
    }

    /// The mouse over the view at `(x, y)`, its button down (a drag).
    fn drag_to(&mut self, label: &str, x: f32, y: f32) {
        let id = self.view(label);
        let e = MouseEvent::new(point(px(x), px(y)), point(px(x), px(y)), Default::default(), Some(MouseButton::Left));
        self.send(id, EventKind::MouseMove, EventPayload::Mouse(e));
        self.pump(Duration::from_millis(50));
    }

    /// A key as the platform takes it: the keymap's binding fires.
    fn key(&mut self, keys: &str) {
        let stroke = Keystroke::parse(keys, Os::MacOs).unwrap();
        let found = self.keymap.as_ref().map_or(KeyMatch::None, |k| k.find(stroke, true, false));
        let KeyMatch::Fire(id) = found else { panic!("nothing binds {keys}") };
        self.send(ROOT_ID, EventKind::KeyCommand, EventPayload::Value(PayloadValue::Number(f64::from(id))));
        self.pump(Duration::from_millis(50));
    }

    /// Characters typed into the input labelled `label`, one edit each.
    fn type_text(&mut self, label: &str, text: &str) {
        let id = self.view(label);
        for c in text.chars() {
            let (mut value, count) = self.inputs.get(&id).cloned().unwrap_or_default();
            value.push(c);
            self.inputs.insert(id, (value.clone(), count + 1));
            self.send(id, EventKind::ChangeText, EventPayload::Text { text: value, count: count + 1 });
            self.pump(Duration::from_millis(50));
        }
    }
}

fn read(app: &Headless) -> HashMap<String, String> {
    parse(app.texts().into_iter().find(|t| t.starts_with("probe ")).unwrap_or_default())
}

fn parse(s: &str) -> HashMap<String, String> {
    s.split(' ').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_owned(), v.to_owned())).collect()
}

fn num(p: &HashMap<String, String>, k: &str) -> f32 {
    p.get(k).and_then(|v| v.parse().ok()).unwrap_or(0.0)
}

/// Waits for the probe to satisfy `ok`.
fn until(app: &mut Headless, what: &str, ok: impl Fn(&HashMap<String, String>) -> bool) -> HashMap<String, String> {
    let end = Instant::now() + WAIT;
    loop {
        let p = read(app);
        if !p.is_empty() && ok(&p) {
            return p;
        }
        assert!(Instant::now() < end, "{what}: never happened\nprobe: {p:?}");
        app.pump(Duration::from_millis(20));
    }
}

/// A Word document shows: laid out, its first page drawn, the font size
/// read and the status bar's page count up.
fn word_shows(app: &mut Headless, what: &str) -> HashMap<String, String> {
    let p = until(app, what, |p| num(p, "pages") > 0.0 && num(p, "drawn") > 0.0 && num(p, "size") > 0.0);
    // the status bar's count
    assert!(app.texts().iter().any(|t| t.starts_with("Page 1 of ")), "{what}: no page count in {:?}", app.texts());
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
    // inch; pages start 16 points down) puts the caret at its end
    app.press_at("Document pages", left + 700.0, 16.0 + 104.0, false);
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
    app.press_at("Document pages", left + 100.0, 16.0 + 104.0, false);
    until(&mut app, "the caret at the line's start", |p| num(p, "sel") == 0.0 && num(p, "cx") < 150.0);
    app.drag_to("Document pages", left + 300.0, 16.0 + 104.0);
    until(&mut app, "the dragged selection", |p| num(p, "sel") > 0.0);
}
