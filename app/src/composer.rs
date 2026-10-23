//! The composer dock (`App.tsx`'s input area): pending attachments, the
//! reply banner, the multiline input with a send button, and the model
//! pill. Submits on Enter (`SubmitBehavior::Submit`).

use nsh_core::ui::prelude::*;

use crate::icons::icon;
use crate::live::store;
use crate::statusfx;
use crate::theme;

/// The bottom dock.
pub(crate) fn composer() -> impl IntoElement {
    let dock_h = signal(22.0f32);
    view()
        .absolute()
        .bottom(px(0.0))
        .left(px(0.0))
        .right(px(0.0))
        .px_4()
        .pb_3()
        .child(reply_banner())
        .child(attachment_strip())
        .child(
            view()
                .flex_row()
                .items_end()
                .gap_2()
                .bg(theme::DOCK)
                .rounded(px(16.0))
                .border_1()
                .border_color(theme::BORDER_S)
                .pl_3()
                .pr_2()
                .py_2()
                .child(attach_button())
                .child(
                    input()
                        .flex_1()
                        .value(move || store().draft.get())
                        .multiline(true)
                        .submit_behavior(if is_desktop() {
                            SubmitBehavior::Submit
                        } else {
                            SubmitBehavior::Newline
                        })
                        .return_key_type(ReturnKeyType::Send)
                        .placeholder("Message…")
                        .placeholder_color(|| color(theme::GHOST))
                        .text_size(px(14.0))
                        .line_height(px(20.0))
                        .text_color(theme::TEXT)
                        .on_change(move |v: &str| store().draft.set(v.to_owned()))
                        .on_content_size_change(move |size: &Size<Points>| {
                            dock_h.set(size.height.clamp(px(22.0), px(160.0)).0);
                        })
                        .on_submit(move |_: &str| store().send())
                        .h(move || px(dock_h.get() + 20.0)),
                )
                .child(send_button()),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .pt_2()
                .child(model_pill())
                .child(view().flex_1())
                .child(when(
                    move || {
                        let st = store();
                        // `agent() = agentRuns()[activeId()]` — the row only
                        // mounts while the run's own thread is open.
                        !st.run_status.get().is_empty()
                            && st.run_thread.get() == st.active_id.get()
                    },
                    || {
                        // `agentSet`'s row: the active bot's mark playing
                        // the run state + the ambient fx + the label.
                        // `seed={agent().bot ?? activeBot()?.id ?? "status"}`.
                        let st = store();
                        let bot = st.active_bot_untracked();
                        let seed = {
                            let rb = st.run_bot.get_untracked();
                            if rb.is_empty() {
                                bot.as_ref().map(|b| b.id.clone())
                            } else {
                                Some(rb)
                            }
                        };
                        let (shape, color) = bot
                            .map(|b| (b.shape, b.color))
                            .unwrap_or_else(|| ("blob".into(), "#FFFFFF".into()));
                        statusfx::status_fx(
                            shape,
                            color,
                            store().run_state,
                            20.0,
                            seed,
                            move || store().run_status.get(),
                        )
                    },
                )),
        )
}

fn model_pill() -> impl IntoElement {
    let hovered = signal(false);
    view()
        .label("Choose model")
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .px_2()
                .py(px(4.0))
                .rounded(px(12.0))
                .bg(move || if hovered.get() { theme::HOVER } else { theme::PILL_BG })
                .border_1()
                .border_color(theme::BORDER_F)
                .child(
                    text(move || {
                        let st = store();
                        let id = st.model.get();
                        st.models
                            .get()
                            .iter()
                            .find(|m| m.id == id)
                            .map(|m| m.label.clone())
                            .unwrap_or(if id.is_empty() { "model".to_owned() } else { id })
                    })
                    .text_size(px(11.0))
                    .text_color(theme::MUTED),
                )
                .child(icon("chev-down")),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(|| store().models_open.set(true))
}

fn send_button() -> impl IntoElement {
    let hovered = signal(false);
    view()
        .label("Send")
        .child(
            view()
                .size(px(32.0))
                .rounded(px(16.0))
                .items_center()
                .justify_center()
                .bg(move || {
                    if hovered.get() {
                        theme::ACCENT
                    } else {
                        theme::CARD2
                    }
                })
                .child(icon("send")),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(|| store().send())
}

fn attach_button() -> impl IntoElement {
    view()
        .label("Attach")
        .child(
            view()
                .size(px(28.0))
                .rounded(px(14.0))
                .items_center()
                .justify_center()
                .child(icon("paperclip")),
        )
        .on_press(|| {
            let st = store();
            st.ctx.set(Some(crate::store::CtxMenu {
                x: 60.0,
                y: 500.0,
                items: vec![crate::store::CtxItem {
                    label: "Attach is host-side".to_owned(),
                    icon: "paperclip".to_owned(),
                    danger: false,
                    act: "noop".to_owned(),
                }],
            }));
        })
}

fn reply_banner() -> impl IntoElement {
    dynamic(|| {
        let Some(r) = store().reply_to.get() else {
            return view().into_element();
        };
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .mb_1()
            .bg(theme::CARD)
            .rounded(px(10.0))
            .border_1()
            .border_color(theme::BORDER_F)
            .child(icon("reply"))
            .child(
                view()
                    .flex_1()
                    .child(
                        text(r.name)
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::MUTED),
                    )
                    .child(
                        text(r.text)
                            .text_size(px(12.0))
                            .text_color(theme::DIM)
                            .number_of_lines(1),
                    ),
            )
            .child(
                view()
                    .label("Cancel reply")
                    .child(icon("close"))
                    .on_press(|| store().reply_to.set(None)),
            )
            .into_element()
    })
}

fn attachment_strip() -> impl IntoElement {
    dynamic(|| {
        let atts = store().attachments.get();
        if atts.is_empty() {
            return view().into_element();
        }
        view()
            .flex_row()
            .flex_wrap()
            .gap_2()
            .pb_2()
            .child(each(
                move || store().attachments.get(),
                |a| a.path.clone(),
                |a_sig, _| {
                    let a = a_sig.get();
                    view()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py(px(4.0))
                        .bg(theme::CARD2)
                        .rounded(px(7.0))
                        .child(icon("folder"))
                        .child(
                            text(if a.name.is_empty() { a.path.clone() } else { a.name.clone() })
                                .text_size(px(11.0))
                                .text_color(theme::MUTED)
                                .number_of_lines(1),
                        )
                },
            ))
            .into_element()
    })
}
