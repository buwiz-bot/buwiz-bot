//! The spreadsheet: `.xlsx`/`.xlsm`, `.xls`, `.csv` and `.tsv`. The
//! workbook lives on the office worker ([`doc`]); this side keeps what the
//! view needs — the shape of the sheet, the display list in view, the
//! selection and the cell being edited — in [`SheetUi`].
//!
//! The app hosts it (`buwiz_office_kit::host`): it keeps the [`SheetUi`]
//! in its store and holds the open workbook's session, both given here
//! through [`FORMAT`] at startup.

#[cfg(test)]
mod bench;
pub mod chart;
pub mod convert;
pub mod doc;
pub mod edit;
#[cfg(any(test, feature = "testing"))]
pub mod fixture;
pub mod grid;
pub mod resize;
pub mod select;

use std::sync::Arc;

use betteroffice_xlsx::{DisplayList, Viewport};
use nsh_core::prelude::*;

use buwiz_office_kit::host::Format;
use doc::{Book, Meta, Out};
use select::Sel;

/// The app's hooks: the view's state in its store, the open workbook's
/// session.
pub static FORMAT: Format<SheetUi, Book> = Format::new();

/// A path's extension, lowercased.
pub fn ext(path: &std::path::Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// [`Meta`] as a signal holds it: a new revision, sheet or engine state
/// is a change.
#[derive(Clone)]
pub struct Shape(pub Arc<Meta>);

impl PartialEq for Shape {
    fn eq(&self, o: &Self) -> bool {
        let (a, b) = (&self.0, &o.0);
        a.rev == b.rev && a.active == b.active && a.editable == b.editable && a.history == b.history && a.sheets == b.sheets && Arc::ptr_eq(&a.geometry, &b.geometry)
    }
}

/// A display list in hand: the sheet window it covers (`vp`, sheet
/// pixels) at `scale`.
#[derive(Clone)]
pub struct Shown {
    pub rev: u64,
    pub sheet: usize,
    pub vp: Viewport,
    pub scale: f32,
    pub list: Arc<DisplayList>,
    /// The merged ranges in the window.
    pub merges: Arc<Vec<doc::Range>>,
}

impl PartialEq for Shown {
    fn eq(&self, o: &Self) -> bool {
        Arc::ptr_eq(&self.list, &o.list)
    }
}

/// The cell being edited and its text so far.
#[derive(Clone, Debug, PartialEq)]
pub struct Editing {
    pub sheet: usize,
    pub cell: (u32, u32),
    pub text: String,
    /// Typed over the cell (arrows commit and move) rather than opened
    /// with F2, a double click or the formula bar (arrows move the caret).
    pub replace: bool,
}

pub struct SheetUi {
    pub shape: Signal<Option<Shape>>,
    pub shown: Signal<Option<Shown>>,
    pub sel: Signal<Sel>,
    /// The body's scroll, in sheet pixels from the first unfrozen cell.
    pub scroll: Signal<(f32, f32)>,
    /// The cells area's size, in points.
    pub area: Signal<(f32, f32)>,
    pub zoom: Signal<f32>,
    pub editing: Signal<Option<Editing>>,
    /// What the formula bar shows for the active cell: (sheet, cell, input).
    pub bar: Signal<(usize, (u32, u32), String)>,
    /// A message from the engine (an edit it refused).
    pub note: Signal<String>,
    /// The view last asked of the worker: (rev, sheet, window, scale).
    pub asked: std::cell::Cell<Option<(u64, usize, Viewport, f32)>>,
    /// The chart selected instead of cells.
    pub chart: Signal<Option<chart::Picked>>,
    /// How far the selected chart's outline is dragged, in points.
    pub chart_drag: Signal<Option<(f32, f32)>>,
    /// The arrows' nudge not sent yet: (frame, dx, dy) in sheet pixels.
    pub nudge: Signal<Option<(String, f32, f32)>>,
    /// A column's or row's border being dragged.
    pub resize: Signal<Option<resize::Drag>>,
}

impl SheetUi {
    #[allow(clippy::new_without_default)]
    pub fn new() -> SheetUi {
        SheetUi {
            shape: signal(None),
            shown: signal(None),
            sel: signal(Sel::default()),
            scroll: signal((0.0, 0.0)),
            area: signal((0.0, 0.0)),
            zoom: signal(1.0),
            editing: signal(None),
            bar: signal((0, (0, 0), String::new())),
            note: signal(String::new()),
            asked: std::cell::Cell::new(None),
            chart: signal(None),
            chart_drag: signal(None),
            nudge: signal(None),
            resize: signal(None),
        }
    }

    /// A new file: nothing from the last one.
    pub fn reset(&self) {
        self.shape.set(None);
        self.shown.set(None);
        self.sel.set(Sel::default());
        self.scroll.set((0.0, 0.0));
        self.editing.set(None);
        self.bar.set((0, (0, 0), String::new()));
        self.note.set(String::new());
        self.asked.set(None);
        self.chart.set(None);
        self.chart_drag.set(None);
        self.nudge.set(None);
        self.resize.set(None);
    }

    /// The file changed under it: the view is drawn again.
    pub fn reloaded(&self) {
        self.editing.set(None);
        self.asked.set(None);
    }

    /// The active sheet (0 before the shape comes).
    pub fn sheet(&self) -> usize {
        self.shape.with_untracked(|s| s.as_ref().map_or(0, |s| s.0.active))
    }
}

/// An answer from the worker.
pub fn on_out(u: &SheetUi, o: Out) {
    match o {
        Out::Meta(m) => {
            let old = u.shape.with_untracked(|s| s.as_ref().map(|s| (s.0.active, s.0.rev)));
            let switched = old.is_some_and(|(a, _)| a != m.active);
            batch(|| {
                if switched {
                    u.sel.set(edit::opened_at(m.active));
                    u.scroll.set((0.0, 0.0));
                    u.editing.set(None);
                    u.chart.set(None);
                    u.chart_drag.set(None);
                    u.nudge.set(None);
                }
                u.shape.set(Some(Shape(Arc::new(m))));
            });
            edit::read_active();
        }
        Out::List { rev, sheet, vp, scale, list, merges } => {
            // a list older than what is shown is dropped (a burst of edits)
            if u.shown.with_untracked(|s| s.as_ref().is_some_and(|s| s.rev > rev)) {
                return;
            }
            u.shown.set(Some(Shown { rev, sheet, vp, scale, list, merges }));
            chart::listed(u);
        }
        Out::Cell { sheet, cell, input } => u.bar.set((sheet, cell, input)),
        Out::Copied { tsv, html } => buwiz_office_kit::clipboard::to_clipboard(&tsv, &html),
        Out::Note(n) => u.note.set(n),
        Out::Fit { sheet, cols } => resize::fit(u, sheet, cols),
    }
}
