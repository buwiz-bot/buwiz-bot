//! A reply's Markdown via `nsh/base`'s `TextView` at chat scale — the
//! Rust port of `app/md.tsx`: tighter headings, code copy action, a
//! streaming caret, selectable blocks.

use nsh_base::{MarkdownOptions, TextViewCodeAction, TextViewPart, TextViewParts, text_view};
use nsh_core::ui::{StyleRefinement, prelude::*};

use crate::theme;

fn parts() -> TextViewParts {
    use TextViewPart as P;
    let heading = |size: f32, line: f32| {
        move |s: StyleRefinement| s.text_size(px(size)).line_height(px(line))
    };
    TextViewParts::new()
        .part(P::Heading1, heading(20.0, 26.0))
        .part(P::Heading2, heading(17.0, 22.0))
        .part(P::Heading3, heading(15.0, 22.0))
        .part(P::Code, |s| s.text_color(theme::ACCENT))
        .part(P::CodeBlockCode, |s| s.text_color(theme::TEXT))
        .part(P::CodeBlock, |s| {
            s.bg(theme::CARD2)
                .rounded(px(8.0))
                .border_1()
                .border_color(theme::BORDER_F)
        })
}

/// The blinking caret that ends a streaming reply's last block.
fn caret() -> impl IntoElement {
    view()
        .w(px(9.0))
        .h(px(17.0))
        .ml(px(2.0))
        .rounded(px(2.0))
        .bg(theme::ACCENT)
        .animation(KeyframeAnimation::new(Keyframes::Pulse).duration(900.0))
}

/// The Markdown `md` gives. While `streaming` holds the document is read
/// as still arriving and the caret ends it. `on_fill` is the text color
/// inside a user bubble (the reply root's own color everywhere else).
pub(crate) fn markdown(
    md: impl Fn() -> String + 'static,
    streaming: impl Fn() -> bool + 'static,
) -> impl IntoElement {
    provide_context(parts());
    let streaming = computed(streaming);
    text_view()
        .markdown(move || Some(SharedString::from(md())))
        .options(move || MarkdownOptions {
            streaming: streaming.get(),
            ..MarkdownOptions::default()
        })
        .selectable(true)
        .code_block_scrollbar(false)
        .code_block_actions(vec![
            TextViewCodeAction::new("copy").with_label("Copy code"),
        ])
        .streaming_caret(when(streaming, caret))
        .gap_3()
        .text_size(px(15.0))
        .line_height(px(21.0))
        .text_color(theme::TEXT)
}
