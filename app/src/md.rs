//! A reply's Markdown via `nsh/base`'s `TextView` at chat scale — the
//! Rust port of `app/md.tsx`: tighter headings, code copy action, a
//! streaming caret, selectable blocks.

use nsh_base::{
    MarkdownExtension, MarkdownOptions, TextViewCodeAction, TextViewPart, TextViewParts,
    text_view::document::CustomNode, text_view,
};
use nsh_core::{element::StyleRefinement, prelude::*};

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

/// Given by a transcript row that mounts out of sight (the pacer's,
/// `chat.rs`): its Markdown comes in [`STAGE_BLOCKS`] top-level blocks
/// per frame instead of in one commit. The count is how many documents
/// are still coming in; the row holds its last height meanwhile.
#[derive(Clone, Copy)]
pub(crate) struct Staged(pub(crate) Signal<u32>);

/// Blocks a staged document adds per step, and the time between steps.
const STAGE_BLOCKS: usize = 4;
const STAGE_MS: u64 = 16;

/// Where `md`'s top-level blocks after the first ones start (byte
/// offsets): at a line that follows a blank line outside a code fence
/// and opens a new block at the margin (not a list item, an indented
/// continuation, or a table row). Cutting there keeps every block whole.
fn block_starts(md: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let (mut fence, mut blank, mut at) = (None::<&str>, false, 0);
    for line in md.split_inclusive('\n') {
        let t = line.trim_end();
        let lead = t.trim_start();
        if let Some(f) = fence {
            if lead.starts_with(f) {
                fence = None;
            }
        } else {
            let opens = !t.is_empty() && !t.starts_with([' ', '\t', '|', '-', '*', '+']) && !t.split_once(['.', ')']).is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
            if blank && opens && at > 0 {
                starts.push(at);
            }
            if lead.starts_with("```") {
                fence = Some("```");
            } else if lead.starts_with("~~~") {
                fence = Some("~~~");
            }
        }
        blank = t.is_empty();
        at += line.len();
    }
    starts
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
    // A staged row's document: its first blocks now, the rest a few per
    // frame (`Staged`); `None` once it is whole.
    let upto = signal(None::<usize>);
    if let Some(Staged(pending)) = use_context::<Staged>()
        && !streaming.get_untracked()
    {
        let blocks = untracked(|| block_starts(&md()).len()) + 1;
        if blocks > STAGE_BLOCKS {
            upto.set(Some(STAGE_BLOCKS));
            pending.update(|n| *n += 1);
            let alive = std::rc::Rc::new(std::cell::Cell::new(true));
            {
                let alive = alive.clone();
                cleanup(move || alive.set(false));
            }
            spawn(async move {
                let mut n = STAGE_BLOCKS;
                while n < blocks {
                    timer(std::time::Duration::from_millis(STAGE_MS)).await;
                    if !alive.get() {
                        return;
                    }
                    n += STAGE_BLOCKS;
                    upto.set((n < blocks).then_some(n));
                }
                pending.update(|n| *n = n.saturating_sub(1));
            });
        }
    }
    text_view()
        .markdown(move || {
            let s = md();
            Some(SharedString::from(match upto.get() {
                Some(n) => block_starts(&s).get(n - 1).map_or(s.clone(), |&at| s[..at].to_owned()),
                None => s,
            }))
        })
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
        // `buwiz://chat/…` links open that chat (and turn) in Rox; the rest
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

#[cfg(test)]
mod tests {
    use super::block_starts;

    #[test]
    fn blocks_are_cut_whole() {
        let md = "# Title\n\nOne para.\n\n- a\n\n- b\n\n```\ncode\n\nmore\n```\n\n| a |\n|---|\n\n1. x\n\nLast.\n";
        let cuts: Vec<&str> = block_starts(md).iter().map(|&at| md[at..].lines().next().unwrap()).collect();
        // a list's next item, a fence's inside, a table and a numbered
        // item are never cut at
        assert_eq!(cuts, ["One para.", "```", "Last."]);
        assert!(block_starts("just one").is_empty());
    }
}
