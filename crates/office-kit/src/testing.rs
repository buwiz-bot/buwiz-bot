//! The office views driven as a person drives them, on a headless engine
//! (`nsh-engine`): the engine and what its batches mounted, presses,
//! drags, touches, keys through the keymap and typing, and a probe — a
//! text starting `probe ` of `key=value` words the tested root keeps
//! current — read back and waited on. The kit's controls are tried with
//! it, and the app's viewer tests drive the whole viewer with it.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use nsh_core::element::Root;
use nsh_core::keymap::{KeyMatch, Keymap, Keystroke};
use nsh_core::platform::{Idiom, Os};
use nsh_core::prelude::*;
use nsh_core::{ApproxTextEngine, EventKind, EventPayload, MountBatch, MountItem, NodeId, PayloadValue, ROOT_ID, TouchEvent, TouchPhase, TouchPoint};
use nsh_engine::{App, Engine, EngineConfig, MountSink};

/// How long [`until`] waits.
pub const WAIT: Duration = Duration::from_secs(20);

thread_local! {
    /// The touches' clock, in milliseconds.
    static CLOCK: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

struct Sink(mpsc::Sender<MountBatch>);

impl MountSink for Sink {
    fn submit(&self, batch: MountBatch) {
        let _ = self.0.send(batch);
    }
}

/// The engine and what its batches mounted: the views alive, their
/// labels, texts, frames and styles, the inputs' values and the keymap.
pub struct Headless {
    engine: Engine,
    rx: mpsc::Receiver<MountBatch>,
    pub live: HashSet<NodeId>,
    pub labels: Vec<(NodeId, String)>,
    texts: HashMap<NodeId, String>,
    inputs: HashMap<NodeId, (String, u32)>,
    keymap: Option<std::sync::Arc<Keymap>>,
    pub frames: HashMap<NodeId, Bounds<Points>>,
    pub styles: HashMap<NodeId, nsh_core::ViewStyle>,
    pub parents: HashMap<NodeId, NodeId>,
    /// The file requests (open dialogs) not answered yet.
    files: Vec<u32>,
}

/// An engine on `root` as a Mac desktop shows it (the office controls'
/// tests: popups as popups, not sheets).
pub fn start_root(root: Root) -> Headless {
    let mut config = EngineConfig::new(App::Rust(root));
    config.viewport = Size::new(px(1200.0), px(900.0));
    config.environment.os = Os::MacOs;
    config.environment.idiom = Idiom::Desktop;
    run(config)
}

/// An engine on `config`, its first render applied.
pub fn run(config: EngineConfig) -> Headless {
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
        files: Vec::new(),
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
                MountItem::Command { command: nsh_core::Command::FileRequest { id, .. }, .. } => self.files.push(*id),
                _ => {}
            }
        }
    }

    /// Applies what comes for `time`.
    pub fn pump(&mut self, time: Duration) {
        let end = Instant::now() + time;
        while let Ok(b) = self.rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            self.apply(&b);
        }
    }

    pub fn texts(&self) -> Vec<&str> {
        self.texts.iter().filter(|(id, _)| self.live.contains(id)).map(|(_, t)| t.as_str()).collect()
    }

    /// The newest live view labelled `label`.
    pub fn view(&self, label: &str) -> NodeId {
        self.labels.iter().rev().find(|(id, l)| l == label && self.live.contains(id)).map(|(id, _)| *id).unwrap_or_else(|| {
            let live: Vec<&str> = self.labels.iter().filter(|(id, _)| self.live.contains(id)).map(|(_, l)| l.as_str()).collect();
            panic!("no view labelled {label:?} in {live:?}")
        })
    }

    pub fn send(&self, id: NodeId, kind: EventKind, payload: EventPayload) {
        self.engine.control().dispatch_event(id, kind, payload);
    }

    /// A click at `(x, y)` in the view labelled `label` (`shift` held).
    pub fn press_at(&mut self, label: &str, x: f32, y: f32, shift: bool) {
        self.click(label, x, y, shift, 1);
    }

    /// The `clicks`th click of a quick run (2: a double click).
    pub fn click(&mut self, label: &str, x: f32, y: f32, shift: bool, clicks: u32) {
        let id = self.view(label);
        let e = PressEvent { position: point(px(x), px(y)), page_position: point(px(x), px(y)), modifiers: nsh_core::keymap::Modifiers { shift, ..Default::default() }, clicks };
        for kind in [EventKind::PressDown, EventKind::PressUp, EventKind::Press] {
            self.send(id, kind, EventPayload::Press(e));
        }
        self.pump(Duration::from_millis(50));
    }

    pub fn press(&mut self, label: &str) {
        self.press_at(label, 2.0, 2.0, false);
    }

    /// The mouse over the view at `(x, y)`, its button down (a drag).
    pub fn drag_to(&mut self, label: &str, x: f32, y: f32) {
        let id = self.view(label);
        let e = MouseEvent::new(point(px(x), px(y)), point(px(x), px(y)), Default::default(), Some(MouseButton::Left));
        self.send(id, EventKind::MouseMove, EventPayload::Mouse(e));
        self.pump(Duration::from_millis(50));
    }

    /// The mouse over the view at `(x, y)`, no button down.
    pub fn hover_at(&mut self, label: &str, x: f32, y: f32) {
        let id = self.view(label);
        let e = MouseEvent::new(point(px(x), px(y)), point(px(x), px(y)), Default::default(), None);
        self.send(id, EventKind::MouseMove, EventPayload::Mouse(e));
        self.pump(Duration::from_millis(50));
    }

    /// The secondary button at `(x, y)` in the view.
    pub fn right_click(&mut self, label: &str, x: f32, y: f32) {
        let id = self.view(label);
        let e = ContextMenuEvent { position: point(px(x), px(y)), page_position: point(px(x), px(y)), modifiers: Default::default() };
        self.send(id, EventKind::ContextMenu, EventPayload::ContextMenu(e));
        self.pump(Duration::from_millis(50));
    }

    /// A finger (the mouse's button, for the touch responder) on the view
    /// labelled `label` at `(x, y)` of the surface, in `phase`.
    pub fn touch(&mut self, label: &str, phase: TouchPhase, x: f32, y: f32) {
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
    pub fn touch_drag(&mut self, label: &str, (x, y): (f32, f32), (dx, dy): (f32, f32), lift: bool) {
        self.touch(label, TouchPhase::Start, x, y);
        for k in 1..=4 {
            let t = k as f32 / 4.0;
            self.touch(label, TouchPhase::Move, x + dx * t, y + dy * t);
        }
        if lift {
            self.touch(label, TouchPhase::End, x + dx, y + dy);
        }
    }

    /// A key as the platform takes it: the keymap's binding fires.
    pub fn key(&mut self, keys: &str) {
        let stroke = Keystroke::parse(keys, Os::MacOs).unwrap();
        let found = self.keymap.as_ref().map_or(KeyMatch::None, |k| k.find(stroke, true, false));
        let KeyMatch::Fire(id) = found else { panic!("nothing binds {keys}") };
        self.send(ROOT_ID, EventKind::KeyCommand, EventPayload::Value(PayloadValue::Number(f64::from(id))));
        self.pump(Duration::from_millis(50));
    }

    /// The newest live view labelled `label` inside the one labelled `root`.
    pub fn view_in(&self, root: &str, label: &str) -> NodeId {
        let root = self.view(root);
        let inside = |mut id: NodeId| {
            while let Some(p) = self.parents.get(&id) {
                if *p == root {
                    return true;
                }
                id = *p;
            }
            false
        };
        self.labels.iter().rev().find(|(id, l)| l == label && self.live.contains(id) && inside(*id)).map(|(id, _)| *id).unwrap_or_else(|| panic!("no {label:?} in {root:?}"))
    }

    /// The pointer onto the view labelled `label` inside `root`.
    pub fn hover_in(&mut self, root: &str, label: &str) {
        let id = self.view_in(root, label);
        let e = MouseEvent::new(point(px(2.0), px(2.0)), point(px(2.0), px(2.0)), Default::default(), None);
        self.send(id, EventKind::MouseEnter, EventPayload::Mouse(e));
        self.pump(Duration::from_millis(50));
    }

    /// The labels inside `root` from left to right (a row's controls in
    /// their order): by their left edge in the window, a container before
    /// what it holds.
    pub fn labels_by_x(&self, root: &str) -> Vec<String> {
        let root_id = self.view(root);
        let x = |mut id: NodeId| {
            let mut x = 0.0;
            while id != root_id {
                x += self.frames.get(&id).map_or(0.0, |f| f.origin.x.0);
                match self.parents.get(&id) {
                    Some(p) => id = *p,
                    None => break,
                }
            }
            x
        };
        let depth = |mut id: NodeId| {
            let mut d = 0;
            while let Some(p) = self.parents.get(&id) {
                d += 1;
                id = *p;
            }
            d
        };
        let names = self.labels_in(root);
        // each view once: where it came, and its label now
        let mut order: Vec<NodeId> = Vec::new();
        let mut now: HashMap<NodeId, String> = HashMap::new();
        for (id, l) in &self.labels {
            if self.live.contains(id) && names.contains(l) {
                if !now.contains_key(id) {
                    order.push(*id);
                }
                now.insert(*id, l.clone());
            }
        }
        let mut ids: Vec<(f32, usize, usize, String)> = order.iter().enumerate().map(|(i, id)| (x(*id), depth(*id), i, now[id].clone())).collect();
        ids.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        ids.into_iter().map(|(_, _, _, l)| l).collect()
    }

    /// The window at `w` × `h`.
    pub fn resize(&mut self, w: f32, h: f32) {
        self.engine.control().resize(Size::new(px(w), px(h)));
        self.pump(Duration::from_millis(100));
    }

    /// Characters typed into the input labelled `label`, one edit each.
    pub fn type_text(&mut self, label: &str, text: &str) {
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
    pub fn has(&self, label: &str) -> bool {
        self.labels.iter().any(|(id, l)| l == label && self.live.contains(id))
    }

    /// The pointer onto (`true`) or off the view labelled `label`.
    pub fn hover(&mut self, label: &str, inside: bool) {
        let id = self.view(label);
        let e = MouseEvent::new(point(px(2.0), px(2.0)), point(px(2.0), px(2.0)), Default::default(), None);
        self.send(id, if inside { EventKind::MouseEnter } else { EventKind::MouseLeave }, EventPayload::Mouse(e));
        self.pump(Duration::from_millis(50));
    }

    /// Escape (or back) as the platform gives it to the overlay labelled
    /// `label`, the top one.
    pub fn cancel(&mut self, label: &str) {
        let id = self.view(label);
        self.send(id, EventKind::Cancel, EventPayload::None);
        self.pump(Duration::from_millis(50));
    }

    /// Return in the input labelled `label`.
    pub fn submit(&mut self, label: &str) {
        let id = self.view(label);
        let (text, count) = self.inputs.get(&id).cloned().unwrap_or_default();
        self.send(id, EventKind::SubmitEditing, EventPayload::Text { text, count });
        self.pump(Duration::from_millis(50));
    }

    /// The frame of the view labelled `label`, in its parent.
    pub fn frame(&self, label: &str) -> Bounds<Points> {
        let id = self.view(label);
        *self.frames.get(&id).unwrap_or_else(|| panic!("no frame for {label:?}"))
    }

    /// The style last sent for the view labelled `label`.
    pub fn style(&self, label: &str) -> nsh_core::ViewStyle {
        let id = self.view(label);
        self.styles.get(&id).cloned().unwrap_or_else(|| panic!("no style for {label:?}"))
    }

    /// The view labelled `label`'s parent's frame.
    pub fn parent_frame(&self, label: &str) -> Bounds<Points> {
        let id = self.view(label);
        self.parents.get(&id).and_then(|p| self.frames.get(p)).copied().unwrap_or_else(|| panic!("no parent frame for {label:?}"))
    }

    /// The last open dialog answered with `path` picked.
    pub fn pick_file(&mut self, path: &Path) {
        let id = self.files.pop().expect("an open dialog is up");
        self.send(ROOT_ID, EventKind::FileRequest, EventPayload::file_answer(id, vec![path.to_owned()]));
        self.pump(Duration::from_millis(100));
    }

    /// The text of the input labelled `label`, as last sent.
    pub fn value(&self, label: &str) -> String {
        let id = self.view(label);
        self.inputs.get(&id).map(|i| i.0.clone()).unwrap_or_default()
    }

    /// The labels of the live views inside the one labelled `root`, in the
    /// order they came.
    pub fn labels_in(&self, root: &str) -> Vec<String> {
        let root = self.view(root);
        let inside = |mut id: NodeId| {
            while let Some(p) = self.parents.get(&id) {
                if *p == root {
                    return true;
                }
                id = *p;
            }
            false
        };
        let mut seen = HashSet::new();
        self.labels.iter().filter(|(id, _)| self.live.contains(id) && inside(*id) && seen.insert(*id)).map(|(_, l)| l.clone()).collect()
    }

    /// The input labelled `label` reports `value` as its whole text (one
    /// edit), whatever it held: as a platform does when keys come faster
    /// than the app answers, or an input method or autocorrect replaces
    /// what was typed.
    pub fn change(&mut self, label: &str, value: &str) {
        let id = self.view(label);
        let count = self.inputs.get(&id).map_or(0, |i| i.1) + 1;
        self.inputs.insert(id, (value.to_owned(), count));
        self.send(id, EventKind::ChangeText, EventPayload::Text { text: value.to_owned(), count });
        self.pump(Duration::from_millis(50));
    }
}

pub fn read(app: &Headless) -> HashMap<String, String> {
    parse(app.texts().into_iter().find(|t| t.starts_with("probe ")).unwrap_or_default())
}

fn parse(s: &str) -> HashMap<String, String> {
    s.split(' ').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_owned(), v.to_owned())).collect()
}

pub fn num(p: &HashMap<String, String>, k: &str) -> f32 {
    p.get(k).and_then(|v| v.parse().ok()).unwrap_or(0.0)
}

/// Waits for the probe to satisfy `ok`.
pub fn until(app: &mut Headless, what: &str, ok: impl Fn(&HashMap<String, String>) -> bool) -> HashMap<String, String> {
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
