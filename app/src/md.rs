//! A reply's Markdown via `nsh/base`'s `TextView` at chat scale — the
//! Rust port of `app/md.tsx`: tighter headings, code copy action, a
//! streaming caret, selectable blocks.

use nsh_base::{
    MarkdownExtension, MarkdownOptions, TextViewCodeAction, TextViewPart, TextViewParts,
    text_view::document::CustomNode, text_view,
};
use nsh_core::ui::{StyleRefinement, prelude::*};

use crate::theme;

fn parts(on_fill: bool) -> TextViewParts {
    use TextViewPart as P;
    let heading = |size: f32, line: f32| {
        move |s: StyleRefinement| s.text_size(px(size)).line_height(px(line))
    };
    // `OnFill` collapses every `tc` to its dark value (theme.ts:34); the
    // inline-code gold `#e8b339` is a literal in md.tsx and stays.
    let d = move |c: AdaptiveColor| if on_fill { theme::always_dark(c) } else { c };
    TextViewParts::new()
        .part(P::Heading1, heading(20.0, 26.0))
        .part(P::Heading2, heading(17.0, 22.0))
        .part(P::Heading3, heading(15.0, 22.0))
        .part(P::Code, |s| s.text_color(theme::tone(
            theme::hsla(0.116, 0.79, 0.57, 1.0),
            theme::hsla(0.116, 0.79, 0.57, 1.0),
        )))
        .part(P::CodeBlockCode, move |s| s.text_color(d(theme::TEXT)))
        .part(P::CodeBlock, move |s| {
            s.bg(d(theme::CARD2))
                .rounded(px(8.0))
                .border_1()
                .border_color(d(theme::BORDER_F))
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
/// as still arriving and the caret ends it. `on_fill` renders the dark
/// ramp in both themes — inside a colored user bubble `OnFill` makes
/// `tc` pass dark values through (theme.ts:34).
pub(crate) fn markdown(
    md: impl Fn() -> String + 'static,
    streaming: impl Fn() -> bool + 'static,
    on_fill: impl Fn() -> bool + 'static,
) -> impl IntoElement {
    // The flag is per-row constant (user → true, agent → false) so parts
    // are built once with the right ramp.
    provide_context(parts(on_fill()));
    let streaming = computed(streaming);
    text_view()
        .markdown(move || Some(SharedString::from(md())))
        // ```mermaid fences become `mermaid` custom nodes, drawn by the
        // web-view block (md.tsx's `MermaidBlock`).
        .extensions(vec![MarkdownExtension::language("mermaid", "mermaid")])
        .render_custom(|node: &CustomNode| {
            if node.name == "mermaid" {
                crate::files::mermaid_block(&node.text).into_element()
            } else {
                view().into_element()
            }
        })
        .options(move || MarkdownOptions {
            streaming: streaming.get(),
            ..MarkdownOptions::default()
        })
        // `rox://chat/…` links open that chat (and turn) in Rox; the rest
        // open outside it.
        .on_link_press(|e: &nsh_base::TextViewLinkPressEvent| store_open_link(&e.url))
        .selectable(true)
        .code_block_scrollbar(false)
        .code_block_actions(vec![
            TextViewCodeAction::new("copy").with_label("Copy code"),
        ])
        .streaming_caret(when(streaming, caret))
        .gap_3()
        // Grok Bot's `.sand-message-prose`: 14px / 22px at weight 420.
        .text_size(px(14.0))
        .line_height(px(22.0))
        .font_weight(FontWeight(420))
        .text_color(move || {
            if on_fill() {
                theme::ON_FILL
            } else {
                theme::TEXT
            }
        })
}

fn store_open_link(url: &str) {
    crate::live::store().open_link(url);
}
