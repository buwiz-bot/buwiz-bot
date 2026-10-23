//! Icon glyphs: Grok Shell's `icons.tsx` draws strokes on canvas; the
//! shell's small controls here are text glyphs sized like them. A canvas
//! port of the stroke set can replace these one by one.

use nsh_core::ui::prelude::*;

use crate::theme;

/// An icon by name (the TSX icon set's names), as a glyph.
pub(crate) fn icon(name: &str) -> impl IntoElement {
    let (ch, size): (&'static str, f32) = match name {
        "plus" => ("+", 16.0),
        "send" => ("↑", 15.0),
        "close" => ("✕", 12.0),
        "dots" => ("···", 14.0),
        "chev-down" => ("⌄", 14.0),
        "chev-right" => ("›", 14.0),
        "reply" => ("↩", 14.0),
        "smile" => ("☺", 14.0),
        "copy" => ("⧉", 14.0),
        "check" => ("✓", 13.0),
        "search" => ("⌕", 14.0),
        "chat" => ("✎", 14.0),
        "box" => ("▢", 13.0),
        "pin" => ("⊙", 13.0),
        "bolt" => ("⚡", 13.0),
        "folder" => ("▤", 14.0),
        "paperclip" => ("⌘", 13.0),
        "settings" => ("⚙", 14.0),
        "agent" => ("◆", 12.0),
        "panel" => ("⇥", 14.0),
        "external" => ("↗", 12.0),
        "trash" => ("✕", 12.0),
        "edit" => ("✎", 13.0),
        "code" => ("</>", 11.0),
        "history" => ("◷", 13.0),
        _ => ("•", 14.0),
    };
    text(ch)
        .text_size(px(size))
        .text_color(theme::DIM)
        .text_center()
}
