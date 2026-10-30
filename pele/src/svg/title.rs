// Port of src/svg/title.ts (the part flowcharts use).

use crate::text::label::{layout_label, Label};
use crate::text::measurer::style;

// Mermaid draws diagram titles as text, whatever they hold.
pub fn title_label(title: Option<&str>, size: f64, max_width: f64) -> Label {
    layout_label(title, false, size, max_width, style::BOLD)
}
