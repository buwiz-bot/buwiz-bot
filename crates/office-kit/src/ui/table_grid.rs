//! The Insert Table hover grid (DR `gc`): cells fill up to the one under
//! the pointer, the size reads under them, a click inserts that many rows
//! and columns. The menu bar's Insert › Table shows it in a submenu, the
//! toolbar's table button (DR `EE`) in a dropdown.

use std::rc::Rc;

use nsh_core::prelude::*;

use super::button::{BUTTON, BUTTON_RADIUS, TOOLBAR_ICON, tooltip_of};
use super::icons::icon;
use super::palette as p;
use super::{Panel, dropdown};

/// The grid's default size (`gridRows = 6`, `gridColumns = 6`).
pub const ROWS: u32 = 6;
pub const COLS: u32 = 6;
/// A cell (`Bd = 18`, `Zh`): 1px `--doc-border` rim, radius 2; `qk = 2`
/// between cells.
pub const CELL: f32 = 18.0;
pub const GAP: f32 = 2.0;

/// The readout (`gc`'s `c`): "{cols} × {rows}", or "Select size" before
/// the pointer is on a cell.
pub fn readout(rows: u32, cols: u32) -> String {
    if rows > 0 && cols > 0 { format!("{cols} \u{d7} {rows}") } else { "Select size".to_owned() }
}

/// The grid, `rows` × `cols`; `on_insert(rows, cols)` on a click.
pub fn table_grid(rows: u32, cols: u32, on_insert: impl Fn(u32, u32) + 'static) -> AnyElement {
    // the cell under the pointer, (0, 0) for none
    let at = signal((0u32, 0u32));
    let on_insert = Rc::new(on_insert);
    view()
        .child(
            view()
                .gap(px(GAP))
                .label("Table size selector")
                .on_mouse_leave(move |_: &MouseEvent| at.set((0, 0)))
                .children((1..=rows).map(|r| {
                    let on_insert = on_insert.clone();
                    view()
                        .flex_row()
                        .gap(px(GAP))
                        .children((1..=cols).map(move |c| {
                            let on_insert = on_insert.clone();
                            let on = move || {
                                let (hr, hc) = at.get();
                                r <= hr && c <= hc
                            };
                            view()
                                .size(px(CELL))
                                .rounded(px(2.0))
                                .border_1()
                                .border_color(move || if on() { p::PRIMARY } else { p::BORDER })
                                .bg(move || if on() { p::PRIMARY } else { p::SURFACE })
                                .label(format!("row {r} column {c}"))
                                .on_mouse_enter(move |_: &MouseEvent| at.set((r, c)))
                                .on_press(move || {
                                    let (hr, hc) = at.get_untracked();
                                    if hr > 0 && hc > 0 {
                                        on_insert(hr, hc)
                                    }
                                })
                                .into_element()
                        }))
                        .into_element()
                })),
        )
        .child(
            // `Zk`: 6 below, 11px medium, centred
            text(move || {
                let (r, c) = at.get();
                readout(r, c)
            })
            .mt(px(6.0))
            .text_size(px(11.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(p::TEXT)
            .text_center(),
        )
        .into_element()
}

/// The toolbar's table button (DR `EE`): a toolbar button opening the
/// grid in a card (radius 8, padding 8).
pub fn insert_table_button(label: &'static str, on_insert: impl Fn(u32, u32) + 'static) -> AnyElement {
    let open = signal(false);
    let hovered = signal(false);
    let anchor = element_ref();
    let on_insert = Rc::new(on_insert);
    view()
        .child(
            view()
                .w(px(BUTTON))
                .h(px(BUTTON))
                .rounded(px(BUTTON_RADIUS))
                .items_center()
                .justify_center()
                .element_ref(&anchor)
                .label(label)
                .bg(move || if hovered.get() { p::TW_ACCENT } else { AdaptiveColor::TRANSPARENT })
                .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
                .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
                .on_press(move || open.update(|o| *o = !*o))
                .tooltip(tooltip_of(label))
                .child(icon("grid_on", TOOLBAR_ICON, move || if hovered.get() { p::TW_ACCENT_FOREGROUND } else { p::TW_MUTED_FOREGROUND })),
        )
        .child(dropdown(anchor, open, format!("{label} grid"), Panel::GRID, move || {
            let on_insert = on_insert.clone();
            table_grid(ROWS, COLS, move |r, c| {
                open.set(false);
                on_insert(r, c)
            })
        }))
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_readout_is_columns_by_rows() {
        assert_eq!(readout(2, 3), "3 \u{d7} 2");
        assert_eq!(readout(0, 0), "Select size");
    }
}
