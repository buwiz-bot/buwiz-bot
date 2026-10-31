//! Charts on the sheet, handled as BetterOffice's web editor handles them
//! (`@betteroffice/xlsx-react`): a press on one selects it instead of the
//! cell under it (the cell selection hides), a drag moves its outline and
//! the release moves the chart, arrows nudge it (1 px, 10 with Shift, sent
//! as one move 250 ms after the last key), Escape drops the unsent nudge and
//! deselects. While one is selected the cell commands are refused with the
//! web's texts; there is no menu, no Delete, and a double click does nothing.

use std::time::Duration;

use betteroffice_xlsx::{ChartRegion, Rect};
use nsh_core::prelude::*;

use super::doc::Cmd;
use super::grid::{self, Frame};
use super::{SheetUi, Shown, edit};

/// The web's nudge: a pixel, ten with Shift, sent this long after the
/// last key.
const NUDGE: f32 = 1.0;
const NUDGE_SHIFT: f32 = 10.0;
const NUDGE_DELAY: Duration = Duration::from_millis(250);

/// The web's refusals (`commands.reasons.unsupportedSelection` and
/// `gestureActive`).
pub const REFUSED: &str = "Not available while a chart is selected.";
pub const MOVING: &str = "Finish moving the chart first.";

/// The selected chart: its frame (`ChartRegion::id`) and whether its
/// anchor can move (an absolute one cannot).
#[derive(Clone, Debug, PartialEq)]
pub struct Picked {
    pub id: String,
    pub movable: bool,
}

fn ui() -> &'static SheetUi {
    crate::FORMAT.ui()
}

fn send(cmd: Cmd) {
    crate::FORMAT.with_session(|s| s.send(cmd));
}

thread_local! {
    /// Bumped by each nudge: a timer finding it changed leaves the send to
    /// the newer one.
    static NUDGES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A chart is selected (tracked): the `$chart` keys apply.
pub fn selected() -> bool {
    edit::sheet_showing() && ui().chart.with(Option::is_some)
}

/// A point of the cells area in the list's coordinates (its own pixels,
/// at its scale): frozen columns and rows stay put, the body is shifted by
/// the scroll's distance into the drawn window.
fn list_point(s: &Shown, f: &Frame, x: f32, y: f32) -> (f32, f32) {
    let (sx, sy) = (x / f.zoom, y / f.zoom);
    let lx = if sx < f.fw { sx } else { sx + f.scroll.0 - s.vp.x };
    let ly = if sy < f.fh { sy } else { sy + f.scroll.1 - s.vp.y };
    (lx * s.scale, ly * s.scale)
}

/// A rect of the list (`clip` says which pane it paints in) in the cells
/// area: (x, y, w, h) in points.
fn area_rect(s: &Shown, f: &Frame, r: &Rect, clip: &Rect) -> (f32, f32, f32, f32) {
    let k = f.zoom / s.scale;
    // a chart in the body is clipped to it: its clip starts past the frozen panes
    let body_x = clip.x / s.scale >= f.fw - 0.5;
    let body_y = clip.y / s.scale >= f.fh - 0.5;
    let x = if body_x { r.x * k + (s.vp.x - f.scroll.0) * f.zoom } else { r.x * k };
    let y = if body_y { r.y * k + (s.vp.y - f.scroll.1) * f.zoom } else { r.y * k };
    (x, y, r.w * k, r.h * k)
}

/// The chart under a point of the cells area: the topmost, as painted.
pub fn at(u: &SheetUi, x: f32, y: f32) -> Option<ChartRegion> {
    let (_, f) = untracked(|| grid::frame(u))?;
    u.shown.with_untracked(|s| {
        let s = s.as_ref()?;
        let (lx, ly) = list_point(s, &f, x, y);
        xlsx_render::chart_at_point(&s.list.charts, lx, ly).cloned()
    })
}

/// Where a chart's hit area (its visible part) is in the cells area
/// (tracked).
pub fn hit_rect(u: &SheetUi, id: &str) -> Option<(f32, f32, f32, f32)> {
    let (_, f) = grid::frame(u)?;
    u.shown.with(|s| {
        let s = s.as_ref()?;
        let c = s.list.charts.iter().find(|c| c.id == id)?;
        Some(area_rect(s, &f, &c.clip, &c.clip))
    })
}

/// Where the selected chart's outline goes (tracked): its whole frame,
/// moved by a drag in progress or the nudge not sent yet.
pub fn outline(u: &SheetUi) -> Option<(f32, f32, f32, f32)> {
    let id = u.chart.with(|c| c.as_ref().map(|c| c.id.clone()))?;
    let (_, f) = grid::frame(u)?;
    let (x, y, w, h) = u.shown.with(|s| {
        let s = s.as_ref()?;
        let c = s.list.charts.iter().find(|c| c.id == id)?;
        Some(area_rect(s, &f, &c.rect, &c.clip))
    })?;
    let (dx, dy) = u.chart_drag.get().unwrap_or((0.0, 0.0));
    let (nx, ny) = u.nudge.with(|n| n.as_ref().filter(|n| n.0 == id).map_or((0.0, 0.0), |n| (n.1, n.2)));
    Some((x + dx + nx * f.zoom, y + dy + ny * f.zoom, w, h))
}

/// A press on `region`: it is selected (the edit in progress is entered,
/// a nudge of another chart sent).
pub fn press(region: &ChartRegion) {
    let u = ui();
    edit::commit_editing();
    flush();
    let picked = Picked { id: region.id.clone(), movable: region.movable };
    if u.chart.get_untracked().as_ref() != Some(&picked) {
        u.chart.set(Some(picked));
    }
    u.chart_drag.set(None);
    edit::focus();
}

/// Back to the cells: the nudge is sent, the chart no longer selected.
pub fn deselect() {
    let u = ui();
    if u.chart.with_untracked(Option::is_none) {
        return;
    }
    flush();
    batch(|| {
        u.chart.set(None);
        u.chart_drag.set(None);
        u.note.set(String::new());
    });
}

/// The selected chart's id, if it can move.
fn movable() -> Option<String> {
    ui().chart.with_untracked(|c| c.as_ref().filter(|c| c.movable).map(|c| c.id.clone()))
}

/// The chart `id` dragged `by` points so far: its outline follows.
pub fn drag(id: &str, by: (f32, f32)) {
    if movable().as_deref() == Some(id) {
        ui().chart_drag.set(Some(by));
    }
}

/// The drag of `id` released `by` points away: the chart moves there (by
/// that in sheet pixels; nothing for no move).
pub fn drop_at(id: &str, by: (f32, f32)) {
    let u = ui();
    u.chart_drag.set(None);
    if movable().as_deref() != Some(id) {
        return;
    }
    let z = u.zoom.get_untracked();
    move_by(id, by.0 / z, by.1 / z);
}

/// The drag lost (the system took the pointer): the chart stays.
pub fn cancel_drag() {
    ui().chart_drag.set(None);
}

fn move_by(id: &str, dx: f32, dy: f32) {
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    let u = ui();
    send(Cmd::MoveChart { sheet: u.sheet(), frame: id.to_owned(), dx, dy });
}

/// An arrow: the outline moves now, the chart once the keys rest.
pub fn nudge(dx: f32, dy: f32, shift: bool) {
    let Some(id) = movable() else { return };
    let u = ui();
    let step = if shift { NUDGE_SHIFT } else { NUDGE };
    // a nudge of another chart goes first
    if u.nudge.with_untracked(|n| n.as_ref().is_some_and(|n| n.0 != id)) {
        flush();
    }
    let (px, py) = u.nudge.with_untracked(|n| n.as_ref().map_or((0.0, 0.0), |n| (n.1, n.2)));
    u.nudge.set(Some((id, px + dx * step, py + dy * step)));
    let n = NUDGES.with(|c| {
        c.set(c.get() + 1);
        c.get()
    });
    drop(set_timeout(NUDGE_DELAY, move || {
        if NUDGES.with(std::cell::Cell::get) == n {
            flush();
        }
    }));
}

/// Sends the nudge not sent yet.
pub fn flush() {
    let u = ui();
    let Some((id, dx, dy)) = u.nudge.get_untracked() else { return };
    u.nudge.set(None);
    move_by(&id, dx, dy);
}

/// Escape: the unsent nudge is dropped, the chart deselected.
pub fn escape() {
    let u = ui();
    NUDGES.with(|c| c.set(c.get() + 1));
    batch(|| {
        u.nudge.set(None);
        u.chart_drag.set(None);
        u.chart.set(None);
        u.note.set(String::new());
    });
}

/// A cell command while a chart is selected: refused with the web's
/// text. False when no chart is selected (the command goes on).
pub fn refused() -> bool {
    let u = ui();
    if u.chart.with_untracked(Option::is_none) {
        return false;
    }
    let text = if u.chart_drag.with_untracked(Option::is_some) { MOVING } else { REFUSED };
    u.note.set(text.to_owned());
    true
}

/// ⌘Z / ⇧⌘Z with a chart selected: as the web's command gate, refused
/// during a drag, else after the nudge is sent.
pub fn undo(redo: bool) {
    let u = ui();
    if u.chart_drag.with_untracked(Option::is_some) {
        u.note.set(MOVING.to_owned());
        return;
    }
    flush();
    edit::undo(redo);
}

/// A new list came: a selected chart it no longer shows is deselected,
/// as the web does when the frame loses it.
pub fn listed(u: &SheetUi) {
    let Some(id) = u.chart.with_untracked(|c| c.as_ref().map(|c| c.id.clone())) else { return };
    let there = u.shown.with_untracked(|s| s.as_ref().is_some_and(|s| s.list.charts.iter().any(|c| c.id == id)));
    if !there {
        deselect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use betteroffice_xlsx::{DisplayList, GridMeta, Viewport};
    use std::sync::Arc;

    fn shown(scale: f32, vp: (f32, f32)) -> Shown {
        let list = DisplayList { width: 0.0, height: 0.0, commands: Vec::new(), grid: GridMeta::default(), hyperlinks: Vec::new(), charts: Vec::new(), truncated: false };
        Shown { rev: 0, sheet: 0, vp: Viewport { x: vp.0, y: vp.1, width: 800.0, height: 600.0 }, scale, list: Arc::new(list), merges: Arc::new(Vec::new()) }
    }

    #[test]
    fn list_points_and_rects_agree_across_scroll_zoom_and_panes() {
        let f = Frame { zoom: 2.0, fw: 50.0, fh: 20.0, scroll: (130.0, 40.0) };
        // drawn from 100, 30 into the body, at the zoom
        let s = shown(2.0, (100.0, 30.0));
        let r = Rect { x: 300.0, y: 200.0, w: 80.0, h: 60.0 };
        let (x, y, w, h) = area_rect(&s, &f, &r, &r);
        assert_eq!((w, h), (80.0, 60.0));
        // the rect's corner maps back onto itself
        assert_eq!(list_point(&s, &f, x, y), (300.0, 200.0));
        // in the frozen corner nothing shifts
        let c = Rect { x: 10.0, y: 4.0, w: 20.0, h: 10.0 };
        assert_eq!(area_rect(&s, &f, &c, &c).0, 10.0);
        assert_eq!(list_point(&s, &f, 10.0, 4.0), (10.0, 4.0));
    }
}
