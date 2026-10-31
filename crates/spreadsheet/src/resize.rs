//! Columns and rows resized as Excel resizes them (the web editor has no
//! headers to drag; the owner chose Excel's way): a band of ±4 points on a
//! header's border takes the resize cursor and the drag, a guide line and
//! a tip ("Width: 14.00 (103 pixels)") follow it, and the release sets the
//! size of that column or row, or of every one selected with it. Width 0
//! hides. A double click on the band fits the columns to their text (rows
//! go back to their automatic height). Each gesture is one change, one
//! undo step.

use betteroffice_xlsx::GridGeometry;
use xlsx_render::geometry::{MAX_DIGIT_WIDTH_PX, col_chars_to_px, row_pt_to_px};

use super::doc::{Cmd, Dim, FitText};
use super::grid::Frame;
use super::select::{LAST_COL, LAST_ROW};
use super::{SheetUi, Shape};

/// How far from a border the pointer takes it, in points.
pub const BAND: f32 = 4.0;
/// Excel's widest column (characters) and tallest row (points).
const MAX_WIDTH: f64 = 255.0;
const MAX_HEIGHT: f64 = 409.5;
/// What a column's text needs past its width: the cell's padding, 2 px a
/// side (BetterOffice's `TEXT_PAD_PX`), which the `#####` rule leaves out.
const TEXT_PADS: f32 = 4.0;

/// A border being dragged: the column's (`cols`) or row's, its size in
/// sheet pixels when the drag began, and how far it went (points).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    pub cols: bool,
    pub index: u32,
    pub from: f32,
    pub by: f32,
}

fn ui() -> &'static SheetUi {
    crate::FORMAT.ui()
}

fn send(cmd: Cmd) {
    crate::FORMAT.with_session(|s| s.send(cmd));
}

/// A width in pixels as Excel stores it: characters of the default
/// font's digit, the 5 px of padding taken out, on the 1/256 grid.
pub fn width_chars(px: f32) -> f64 {
    let mdw = MAX_DIGIT_WIDTH_PX;
    ((((px as f64).round() - 5.0) / mdw * 256.0).trunc() / 256.0).clamp(0.0, MAX_WIDTH)
}

/// A height in pixels as points, on whole pixels as Excel drags them.
pub fn height_pt(px: f32) -> f64 {
    ((px as f64).round() * 0.75).clamp(0.0, MAX_HEIGHT)
}

/// The tip over a drag: Excel's.
pub fn tip(cols: bool, px: f32) -> String {
    if cols {
        let chars = width_chars(px);
        format!("Width: {chars:.2} ({} pixels)", col_chars_to_px(chars).round())
    } else {
        let pt = height_pt(px);
        format!("Height: {pt:.2} ({} pixels)", row_pt_to_px(pt).round())
    }
}

/// The size of column (`cols`) or row `i`, in sheet pixels.
fn size_of(g: &GridGeometry, cols: bool, i: u32) -> f32 {
    if cols { g.col_x(i + 1) - g.col_x(i) } else { g.row_y(i + 1) - g.row_y(i) }
}

/// The border near `at` (points along the header, as the cells area
/// counts them): the column or row it ends, or `None` past [`BAND`]. At
/// a hidden one's border it is the visible one before it, as in Excel.
pub fn border_at(g: &GridGeometry, f: &Frame, cols: bool, at: f32) -> Option<u32> {
    let (sx, sy) = f.to_sheet(at, at);
    let i = if cols { g.col_at_x(sx).min(LAST_COL) } else { g.row_at_y(sy).min(LAST_ROW) };
    let edge = |i: u32| if cols { f.screen_x(g.col_x(i)) } else { f.screen_y(g.row_y(i)) };
    let mut end = if (edge(i + 1) - at).abs() <= BAND {
        i
    } else if i > 0 && (edge(i) - at).abs() <= BAND {
        i - 1
    } else {
        return None;
    };
    while end > 0 && size_of(g, cols, end) <= 0.0 {
        end -= 1;
    }
    Some(end)
}

/// The columns (`cols`) or rows a change to `i` applies to: every one of
/// a whole-column (whole-row) selection holding it, else just it. A
/// selection to the sheet's edge stops at the used range.
pub fn targets(shape: &Shape, cols: bool, i: u32) -> Vec<u32> {
    let ((r0, c0), (r1, c1)) = ui().sel.get_untracked().range();
    let (whole, a, b, last, used) = if cols { (r0 == 0 && r1 == LAST_ROW, c0, c1, LAST_COL, shape.0.used.1) } else { (c0 == 0 && c1 == LAST_COL, r0, r1, LAST_ROW, shape.0.used.0) };
    if !whole || !(a..=b).contains(&i) {
        return vec![i];
    }
    let b = if b == last { b.min(used.max(i + 1) - 1) } else { b };
    (a..=b).collect()
}

/// The drag starts on the border ending column (`cols`) or row `i`.
pub fn start(cols: bool, i: u32) {
    let u = ui();
    let Some(shape) = u.shape.get_untracked() else { return };
    super::chart::deselect();
    super::edit::commit_editing();
    u.resize.set(Some(Drag { cols, index: i, from: size_of(&shape.0.geometry, cols, i), by: 0.0 }));
}

pub fn drag(by: f32) {
    let u = ui();
    if let Some(d) = u.resize.get_untracked() {
        u.resize.set(Some(Drag { by, ..d }));
    }
}

/// The new size of a drag, in sheet pixels.
pub fn dragged_px(d: &Drag, zoom: f32) -> f32 {
    (d.from + d.by / zoom).max(0.0)
}

/// The release: the size is set (nothing for a click).
pub fn drop_at(by: f32) {
    let u = ui();
    let Some(d) = u.resize.get_untracked() else { return };
    u.resize.set(None);
    let Some(shape) = u.shape.get_untracked() else { return };
    if by.abs() < 0.5 {
        return;
    }
    let px = dragged_px(&Drag { by, ..d }, u.zoom.get_untracked());
    let size = if d.cols { width_chars(px) } else { height_pt(px) };
    let dims = targets(&shape, d.cols, d.index).into_iter().map(|i| (i, Dim::Size(Some(size)))).collect();
    send(Cmd::Dims { sheet: u.sheet(), cols: d.cols, dims });
}

pub fn cancel() {
    ui().resize.set(None);
}

/// A double click on the border: columns fit their text (the worker sends
/// it, [`fit`] measures), rows take their automatic height.
pub fn autofit(cols: bool, i: u32) {
    let u = ui();
    u.resize.set(None);
    let Some(shape) = u.shape.get_untracked() else { return };
    let which = targets(&shape, cols, i);
    if cols {
        send(Cmd::Fit { sheet: u.sheet(), cols: which });
    } else {
        send(Cmd::Dims { sheet: u.sheet(), cols: false, dims: which.into_iter().map(|r| (r, Dim::Size(None))).collect() });
    }
}

/// The widest text of each column measured as the grid draws it (its
/// font, plus the cell's padding and indent), and the widths set to fit.
/// A column with no text keeps its width.
pub fn fit(u: &SheetUi, sheet: usize, cols: Vec<(u32, Vec<FitText>)>) {
    let measure = nsh_core::element::text_measurer();
    let dims: Vec<(u32, Dim)> = cols
        .into_iter()
        .filter(|(_, texts)| !texts.is_empty())
        .map(|(c, texts)| {
            let px = texts.iter().map(|t| measure(&t.font, &t.text).0 + TEXT_PADS + t.indent as f32 * 3.0 * measure(&t.font, " ").0).fold(0.0f32, f32::max);
            (c, Dim::Size(Some(fit_chars(px))))
        })
        .collect();
    if !dims.is_empty() && u.sheet() == sheet {
        send(Cmd::Dims { sheet, cols: true, dims });
    }
}

/// The narrowest width in characters whose pixels hold `px`.
pub fn fit_chars(px: f32) -> f64 {
    ((((px as f64) - 5.0) / MAX_DIGIT_WIDTH_PX * 256.0).ceil() / 256.0).clamp(0.0, MAX_WIDTH)
}

/// Hide or show the columns (`cols`) or rows selected; Unhide shows the
/// hidden ones among them.
pub fn hide(cols: bool, hidden: bool) {
    let u = ui();
    let Some(shape) = u.shape.get_untracked() else { return };
    let g = &shape.0.geometry;
    let ((r0, c0), (r1, c1)) = u.sel.get_untracked().range();
    let (a, b, last, used) = if cols { (c0, c1, LAST_COL, shape.0.used.1) } else { (r0, r1, LAST_ROW, shape.0.used.0) };
    let b = if b == last { b.min(used.max(a + 1) - 1) } else { b };
    let dims = (a..=b).filter(|&i| hidden || size_of(g, cols, i) <= 0.0).map(|i| (i, Dim::Hidden(hidden))).collect();
    send(Cmd::Dims { sheet: u.sheet(), cols, dims });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_and_heights_go_by_excels_units() {
        // Excel's own numbers: 103 px is 14 characters, 64 px the default
        assert_eq!(width_chars(103.0), 14.0);
        assert_eq!(tip(true, 103.0), "Width: 14.00 (103 pixels)");
        assert_eq!(tip(true, 100.0), "Width: 13.57 (100 pixels)");
        assert_eq!(tip(false, 20.0), "Height: 15.00 (20 pixels)");
        // dragged to nothing: hidden
        assert_eq!(width_chars(3.0), 0.0);
        assert_eq!(height_pt(-4.0), 0.0);
        assert_eq!(height_pt(1000.0), 409.5);
        // a fit holds what it was asked for
        for px in [11.0f32, 64.4, 87.2, 99.0, 250.7] {
            assert!(col_chars_to_px(fit_chars(px)) >= px, "{px}");
        }
    }
}
