//! The bar over a document asking about its edits: the file changed
//! outside the app while it held them, or closing asks what to do with
//! them. The Word view and the spreadsheet both show it.

use nsh_core::prelude::*;

use crate::host::{host, theme};

/// A bar over the document: the file changed outside Rox while it held
/// edits, or closing asks what to do with them.
pub fn prompts() -> impl IntoElement {
    let button = |label: &'static str, primary: bool, on: fn()| {
        view()
            .label(label)
            .h(px(26.0))
            .px_3()
            .rounded(px(6.0))
            .justify_center()
            .bg(if primary { theme().accent } else { theme().hover })
            .child(text(label).text_size(px(12.0)).text_color(if primary { theme().on_fill } else { theme().text }))
            .on_press(on)
    };
    dynamic(move || {
        let h = host();
        let conflict = h.state().with(|s| s.conflict);
        let ask = h.ask_close().get();
        if !conflict && !ask {
            return view().into_element();
        }
        let name = untracked(|| h.viewing_name()).unwrap_or_default();
        let row = view().flex_row().items_center().gap_2().px_3().py_2().bg(theme().accent_tint).border_b(px(0.5)).border_b_color(theme().border_s);
        if ask {
            row.child(text(format!("Save the changes to {name}?")).flex_1().text_size(px(13.0)).text_color(theme().text))
                .child(button("Don't Save", false, || host().answer_close(Some(false))))
                .child(button("Cancel", false, || host().answer_close(None)))
                .child(button("Save", true, || host().answer_close(Some(true))))
                .into_element()
        } else {
            row.child(text(format!("{name} changed outside Buwiz Bot while you were editing it.")).flex_1().text_size(px(13.0)).text_color(theme().text))
                .child(button("Keep Mine", false, || host().keep_mine()))
                .child(button("Reload", true, || host().reload()))
                .into_element()
        }
    })
}
