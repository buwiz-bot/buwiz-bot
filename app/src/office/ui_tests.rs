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
use nsh_core::platform::{Idiom, Os};
use nsh_core::prelude::*;
use nsh_core::{ApproxTextEngine, EventKind, EventPayload, MountBatch, MountItem, NodeId, PayloadValue, ROOT_ID, TouchEvent, TouchPhase, TouchPoint};
use nsh_engine::{App, Engine, EngineConfig, MountSink};

use crate::live::store;
use crate::store::Attach;

/// Set in the child process that runs a test's body.
const CHILD: &str = "ROX_OFFICE_UI_TEST";
const WAIT: Duration = Duration::from_secs(20);

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
        .env("ROX_BACKEND", "http://127.0.0.1:1")
        // a reply's files: under the token's folder
        .env("ROX_HOST_TOKEN_FILE", broker_dir(name).join("token"))
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
    let left = m.pages.first().map_or(0.0, |p| ((w.area.get_untracked().0 - p.0 * z) / 2.0).max(16.0));
    let (cp, cx, cy) = c.caret.map_or((-1, 0.0, 0.0), |(p, x, y, _)| (p as i64, x, y));
    let sheet = &o.sheet;
    format!(
        "probe viewing={} switch={} key={} pages={} drawn={drawn} size={} rev={} bold={} caret={cp} cx={cx} cy={cy} sel={} left={left} shape={} cells={} dirty={}",
        if store().viewing.get_untracked().is_some() { "open" } else { "-" },
        SWITCHED.with_borrow(|s| s.replace(' ', "_")),
        if o.key.get_untracked().is_empty() { "-" } else { "open" },
        m.pages.len(),
        m.size,
        m.rev,
        m.bold,
        c.sel.len(),
        sheet.shape.with_untracked(Option::is_some),
        sheet.shown.with_untracked(|s| s.is_some() as u8),
        o.state.get_untracked().dirty,
    ) + &sheet_probe()
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
    /// The touches' clock, in milliseconds.
    static CLOCK: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
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

struct Sink(mpsc::Sender<MountBatch>);

impl MountSink for Sink {
    fn submit(&self, batch: MountBatch) {
        let _ = self.0.send(batch);
    }
}

/// The engine and what its batches mounted: the views alive, their
/// labels, texts, frames and styles, the inputs' values and the keymap.
pub(crate) struct Headless {
    engine: Engine,
    rx: mpsc::Receiver<MountBatch>,
    live: HashSet<NodeId>,
    labels: Vec<(NodeId, String)>,
    texts: HashMap<NodeId, String>,
    inputs: HashMap<NodeId, (String, u32)>,
    keymap: Option<std::sync::Arc<Keymap>>,
    pub(crate) frames: HashMap<NodeId, Bounds<Points>>,
    pub(crate) styles: HashMap<NodeId, nsh_core::ViewStyle>,
    pub(crate) parents: HashMap<NodeId, NodeId>,
}

fn start(files: Vec<Attach>) -> Headless {
    let _ = FILES.set(files);
    let mut config = EngineConfig::new(App::Rust(Root::new::<Viewer>(|| ())));
    config.viewport = Size::new(px(1200.0), px(900.0));
    run(config)
}

/// An engine on `root` as a Mac desktop shows it (the office controls'
/// tests: popups as popups, not sheets).
pub(crate) fn start_root(root: Root) -> Headless {
    let mut config = EngineConfig::new(App::Rust(root));
    config.viewport = Size::new(px(1200.0), px(900.0));
    config.environment.os = Os::MacOs;
    config.environment.idiom = Idiom::Desktop;
    run(config)
}

fn run(config: EngineConfig) -> Headless {
    let (tx, rx) = mpsc::channel();
    let engine = Engine::start(config, || ApproxTextEngine, Sink(tx)).expect("engine starts");
    let mut app = Headless {
        engine,
        rx,
        live: HashSet::new(),
        labels: Vec::new(),
        texts: HashMap::new(),
        inputs: HashMap::new(),
        keymap: None,
        frames: HashMap::new(),
        styles: HashMap::new(),
        parents: HashMap::new(),
    };
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
                MountItem::AccessibilityLabel { id, label: Some(l) } => self.labels.push((*id, l.to_string())),
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
                MountItem::Frame { id, frame } => {
                    self.frames.insert(*id, *frame);
                }
                MountItem::ViewStyle { id, style } => {
                    self.styles.insert(*id, (**style).clone());
                }
                MountItem::Insert { parent, child, .. } => {
                    self.parents.insert(*child, *parent);
                }
                _ => {}
            }
        }
    }

    /// Applies what comes for `time`.
    pub(crate) fn pump(&mut self, time: Duration) {
        let end = Instant::now() + time;
        while let Ok(b) = self.rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            self.apply(&b);
        }
    }

    pub(crate) fn texts(&self) -> Vec<&str> {
        self.texts.iter().filter(|(id, _)| self.live.contains(id)).map(|(_, t)| t.as_str()).collect()
    }

    /// The newest live view labelled `label`.
    pub(crate) fn view(&self, label: &str) -> NodeId {
        self.labels.iter().rev().find(|(id, l)| l == label && self.live.contains(id)).map(|(id, _)| *id).unwrap_or_else(|| {
            let live: Vec<&str> = self.labels.iter().filter(|(id, _)| self.live.contains(id)).map(|(_, l)| l.as_str()).collect();
            panic!("no view labelled {label:?} in {live:?}")
        })
    }

    pub(crate) fn send(&self, id: NodeId, kind: EventKind, payload: EventPayload) {
        self.engine.control().dispatch_event(id, kind, payload);
    }

    /// A click at `(x, y)` in the view labelled `label` (`shift` held).
    pub(crate) fn press_at(&mut self, label: &str, x: f32, y: f32, shift: bool) {
        self.click(label, x, y, shift, 1);
    }

    /// The `clicks`th click of a quick run (2: a double click).
    pub(crate) fn click(&mut self, label: &str, x: f32, y: f32, shift: bool, clicks: u32) {
        let id = self.view(label);
        let e = PressEvent { position: point(px(x), px(y)), page_position: point(px(x), px(y)), modifiers: nsh_core::keymap::Modifiers { shift, ..Default::default() }, clicks };
        for kind in [EventKind::PressDown, EventKind::PressUp, EventKind::Press] {
            self.send(id, kind, EventPayload::Press(e));
        }
        self.pump(Duration::from_millis(50));
    }

    pub(crate) fn press(&mut self, label: &str) {
        self.press_at(label, 2.0, 2.0, false);
    }

    /// The mouse over the view at `(x, y)`, its button down (a drag).
    fn drag_to(&mut self, label: &str, x: f32, y: f32) {
        let id = self.view(label);
        let e = MouseEvent::new(point(px(x), px(y)), point(px(x), px(y)), Default::default(), Some(MouseButton::Left));
        self.send(id, EventKind::MouseMove, EventPayload::Mouse(e));
        self.pump(Duration::from_millis(50));
    }

    /// The mouse over the view at `(x, y)`, no button down.
    fn hover_at(&mut self, label: &str, x: f32, y: f32) {
        let id = self.view(label);
        let e = MouseEvent::new(point(px(x), px(y)), point(px(x), px(y)), Default::default(), None);
        self.send(id, EventKind::MouseMove, EventPayload::Mouse(e));
        self.pump(Duration::from_millis(50));
    }

    /// The secondary button at `(x, y)` in the view.
    fn right_click(&mut self, label: &str, x: f32, y: f32) {
        let id = self.view(label);
        let e = ContextMenuEvent { position: point(px(x), px(y)), page_position: point(px(x), px(y)), modifiers: Default::default() };
        self.send(id, EventKind::ContextMenu, EventPayload::ContextMenu(e));
        self.pump(Duration::from_millis(50));
    }

    /// A finger (the mouse's button, for the touch responder) on the view
    /// labelled `label` at `(x, y)` of the surface, in `phase`.
    fn touch(&mut self, label: &str, phase: TouchPhase, x: f32, y: f32) {
        let id = self.view(label);
        let at = point(px(x), px(y));
        let time = CLOCK.with(|c| {
            c.set(c.get() + 16.0);
            c.get()
        });
        let p = TouchPoint { id: 1, target: id, path: vec![id], page: at, location: at, changed: true };
        let e = TouchEvent { phase, timestamp: time, points: vec![p], activated: None };
        self.send(ROOT_ID, EventKind::Touch, EventPayload::Touch(Box::new(e)));
        self.pump(Duration::from_millis(50));
    }

    /// A drag on the view from `(x, y)` by `(dx, dy)`, in four steps, the
    /// button released at the end (`lift`) or held.
    fn touch_drag(&mut self, label: &str, (x, y): (f32, f32), (dx, dy): (f32, f32), lift: bool) {
        self.touch(label, TouchPhase::Start, x, y);
        for k in 1..=4 {
            let t = k as f32 / 4.0;
            self.touch(label, TouchPhase::Move, x + dx * t, y + dy * t);
        }
        if lift {
            self.touch(label, TouchPhase::End, x + dx, y + dy);
        }
    }

    /// The row labelled `label` of the open menu, picked (on the engine's
    /// thread, by the `pick` button).
    fn pick(&mut self, label: &str) {
        *PICK.lock().unwrap() = label.to_owned();
        self.press("pick");
    }

    /// A key as the platform takes it: the keymap's binding fires.
    pub(crate) fn key(&mut self, keys: &str) {
        let stroke = Keystroke::parse(keys, Os::MacOs).unwrap();
        let found = self.keymap.as_ref().map_or(KeyMatch::None, |k| k.find(stroke, true, false));
        let KeyMatch::Fire(id) = found else { panic!("nothing binds {keys}") };
        self.send(ROOT_ID, EventKind::KeyCommand, EventPayload::Value(PayloadValue::Number(f64::from(id))));
        self.pump(Duration::from_millis(50));
    }

    /// Characters typed into the input labelled `label`, one edit each.
    pub(crate) fn type_text(&mut self, label: &str, text: &str) {
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

impl Headless {
    /// Whether a view labelled `label` is mounted.
    pub(crate) fn has(&self, label: &str) -> bool {
        self.labels.iter().any(|(id, l)| l == label && self.live.contains(id))
    }

    /// The pointer onto (`true`) or off the view labelled `label`.
    pub(crate) fn hover(&mut self, label: &str, inside: bool) {
        let id = self.view(label);
        let e = MouseEvent::new(point(px(2.0), px(2.0)), point(px(2.0), px(2.0)), Default::default(), None);
        self.send(id, if inside { EventKind::MouseEnter } else { EventKind::MouseLeave }, EventPayload::Mouse(e));
        self.pump(Duration::from_millis(50));
    }

    /// Escape (or back) as the platform gives it to the overlay labelled
    /// `label`, the top one.
    pub(crate) fn cancel(&mut self, label: &str) {
        let id = self.view(label);
        self.send(id, EventKind::Cancel, EventPayload::None);
        self.pump(Duration::from_millis(50));
    }

    /// Return in the input labelled `label`.
    pub(crate) fn submit(&mut self, label: &str) {
        let id = self.view(label);
        let (text, count) = self.inputs.get(&id).cloned().unwrap_or_default();
        self.send(id, EventKind::SubmitEditing, EventPayload::Text { text, count });
        self.pump(Duration::from_millis(50));
    }

    /// The frame of the view labelled `label`, in its parent.
    pub(crate) fn frame(&self, label: &str) -> Bounds<Points> {
        let id = self.view(label);
        *self.frames.get(&id).unwrap_or_else(|| panic!("no frame for {label:?}"))
    }

    /// The style last sent for the view labelled `label`.
    pub(crate) fn style(&self, label: &str) -> nsh_core::ViewStyle {
        let id = self.view(label);
        self.styles.get(&id).cloned().unwrap_or_else(|| panic!("no style for {label:?}"))
    }

    /// The view labelled `label`'s parent's frame.
    pub(crate) fn parent_frame(&self, label: &str) -> Bounds<Points> {
        let id = self.view(label);
        self.parents.get(&id).and_then(|p| self.frames.get(p)).copied().unwrap_or_else(|| panic!("no parent frame for {label:?}"))
    }

    /// The input labelled `label` reports `value` as its whole text (one
    /// edit), whatever it held: as a platform does when keys come faster
    /// than the app answers, or an input method or autocorrect replaces
    /// what was typed.
    pub(crate) fn change(&mut self, label: &str, value: &str) {
        let id = self.view(label);
        let count = self.inputs.get(&id).map_or(0, |i| i.1) + 1;
        self.inputs.insert(id, (value.to_owned(), count));
        self.send(id, EventKind::ChangeText, EventPayload::Text { text: value.to_owned(), count });
        self.pump(Duration::from_millis(50));
    }
}

pub(crate) fn read(app: &Headless) -> HashMap<String, String> {
    parse(app.texts().into_iter().find(|t| t.starts_with("probe ")).unwrap_or_default())
}

fn parse(s: &str) -> HashMap<String, String> {
    s.split(' ').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_owned(), v.to_owned())).collect()
}

pub(crate) fn num(p: &HashMap<String, String>, k: &str) -> f32 {
    p.get(k).and_then(|v| v.parse().ok()).unwrap_or(0.0)
}

/// Waits for the probe to satisfy `ok`.
pub(crate) fn until(app: &mut Headless, what: &str, ok: impl Fn(&HashMap<String, String>) -> bool) -> HashMap<String, String> {
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
    app.press_at("Document pages", left + 700.0, 16.0 + 104.0, false);
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
    let p = read(&app);
    assert_eq!(word(&p, "nudge"), "3,10", "{p:?}");
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
