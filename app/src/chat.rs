//! The transcript (`TurnCell` in `shell.tsx`): a `virtual_list` over the
//! open thread's turns — user bubbles on the right, agent markdown on the
//! left, reply quotes, reactions, and generated-app cards.

use nsh_core::ui::prelude::*;

use crate::icons::icon;
use crate::live::store;
use crate::md::markdown;
use crate::store::{Attach, Turn};
use crate::theme;
use crate::widgets::app_card;

/// The scrollable conversation. `follow_end` keeps the view pinned while
/// a reply streams in.
pub(crate) fn transcript() -> impl IntoElement {
    let st = store();
    let list = element_ref();
    // Poll turns while the thread is open.
    set_interval(std::time::Duration::from_millis(1500), move || st.refresh_turns());
    view().flex_1().child(
        virtual_list(
            move || store().turns.get().len(),
            move |at| turn_row(store().turns.get_untracked()[at.index()].clone()),
        )
        .keys(move || {
            store().turns.get().iter().map(|t| t.i.to_string()).collect::<Vec<_>>()
        })
        .follow_end(px(40.0))
        .element_ref(&list)
        .px_4()
        .pt(px(56.0)) // under the top overlay bar
        .pb_4(),
    )
}

/// The empty state when no thread is open or no turns exist.
pub(crate) fn empty_state() -> impl IntoElement {
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .child(
            text("Rox")
                .text_size(px(28.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::GHOST),
        )
        .child(
            text("Ask anything.")
                .text_size(px(13.0))
                .text_color(theme::GHOST)
                .pt_1(),
        )
}

fn turn_row(t: Turn) -> impl IntoElement {
    let user = t.role == "user";
    if user {
        user_row(t).into_element()
    } else {
        agent_row(t).into_element()
    }
}

// ---------------------------------------------------------------------------
// User bubble
// ---------------------------------------------------------------------------

fn user_row(t: Turn) -> impl IntoElement {
    let body = t.text.clone();
    let reply = t.reply.clone();
    let atts = t.attachments.clone();
    let files = t.files.clone();
    view()
        .flex_row()
        .justify_end()
        .py(px(3.0))
        .child(
            view()
                .max_w(px(520.0))
                .bg(theme::BUBBLE)
                .rounded(px(14.0))
                .px_3()
                .py_2()
                .child(quote_block(reply))
                .child(markdown(move || body.clone(), || false))
                .child(attach_strip_parts(atts, files)),
        )
}

// ---------------------------------------------------------------------------
// Agent row
// ---------------------------------------------------------------------------

fn agent_row(t: Turn) -> impl IntoElement {
    let name = t.name.clone();
    let caption = t.caption.clone();
    let body = t.text.clone();
    let streaming = t.building;
    let i = t.i;
    let reaction = t.reaction.clone();
    let has_card = t.art.is_some();
    view()
        .py(px(3.0))
        .max_w(px(640.0))
        .child(
            // Sender line: name + caption.
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .when(!name.is_empty() || !caption.is_empty(), |v| {
                    v.child(
                        text(name.clone())
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::FAINT),
                    )
                    .child(
                        text(caption.clone())
                            .text_size(px(11.0))
                            .text_color(theme::GHOST)
                            .number_of_lines(1),
                    )
                }),
        )
        .child(reply_quote(&t))
        .child(markdown(move || body.clone(), move || streaming))
        .child(attach_strip(&t))
        .when(has_card, |v| v.child(app_card(t.art.clone().unwrap())))
        .child(
            // Action bar: reply / react / more.
            view()
                .flex_row()
                .gap_1()
                .pt_1()
                .child(action("reply", "Reply", move || {
                    let st = store();
                    let tr = st.turns.get_untracked();
                    if let Some(tt) = tr.iter().find(|x| x.i == i) {
                        st.reply_to.set(Some(crate::store::ReplyTo {
                            i: tt.i,
                            name: tt.name.clone(),
                            text: tt.text.chars().take(120).collect(),
                        }));
                    }
                }))
                .child(action("smile", "React", move || {
                    store().react(i, "👍".to_owned());
                }))
                .when(!reaction.is_empty(), move |v| {
                    v.child(
                        text(reaction.clone())
                            .text_size(px(12.0))
                            .px_2()
                            .py(px(2.0))
                            .bg(theme::CARD2)
                            .rounded(px(10.0)),
                    )
                }),
        )
}

fn action(
    name: &'static str,
    label: &'static str,
    on: impl Fn() + 'static,
) -> impl IntoElement {
    let hovered = signal(false);
    view()
        .label(label)
        .child(
            view()
                .size(px(26.0))
                .rounded(px(6.0))
                .items_center()
                .justify_center()
                .bg(move || if hovered.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
                .child(icon(name)),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || on())
}

fn reply_quote(t: &Turn) -> impl IntoElement {
    quote_block(t.reply.clone())
}

fn quote_block(reply: Option<crate::store::Reply>) -> impl IntoElement {
    dynamic(move || {
        let Some(r) = reply.clone() else {
            return view().into_element();
        };
        view()
            .border_l(px(2.0))
            .border_l_color(theme::BORDER_S)
            .pl_2()
            .mb_1()
            .child(
                text(r.name)
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::MUTED),
            )
            .child(
                text(r.text)
                    .text_size(px(12.0))
                    .text_color(theme::MUTED)
                    .number_of_lines(2),
            )
            .into_element()
    })
}

fn attach_strip(t: &Turn) -> impl IntoElement {
    attach_strip_parts(t.attachments.clone(), t.files.clone())
}

fn attach_strip_parts(atts: Vec<Attach>, files: Vec<Attach>) -> impl IntoElement {
    let all: Vec<Attach> = atts.into_iter().chain(files).collect();
    if all.is_empty() {
        return view().into_element();
    }
    view()
        .flex_row()
        .flex_wrap()
        .gap_2()
        .pt_1()
        .child(each(
            move || all.clone(),
            |f| f.path.clone(),
            |f_sig, _| {
                let f = f_sig.get();
                crate::files::file_tile(f).into_element()
            },
        ))
        .into_element()
}
