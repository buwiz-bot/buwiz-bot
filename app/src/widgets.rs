//! Generated-app cards (`WidgetShelf`/`TurnCell`'s widget branch in the
//! TSX). The TSX runs server-emitted TSX inside QuickJS; under a Rust
//! root there is no script engine, so the card is a native stand-in:
//! title, version, open-state, and the actions that pin it to the thread
//! or open its detail in the right pane.

use nsh_core::ui::prelude::*;

use crate::icons::icon;
use crate::live::store;
use crate::store::Art;
use crate::theme;

/// The card a `widget` turn renders: a native placeholder for the app
/// body plus the action bar. `art.app` names the generated module.
pub(crate) fn app_card(art: Art) -> impl IntoElement {
    let app = art.app.clone();
    let app2 = art.app.clone();
    // The model declares the size (`art.w`/`art.h`); the card is capped
    // like the TSX (`min(w,560)`, aspect ratio for the body height).
    let card_w = if art.w > 0.0 { (art.w as f32).min(560.0) } else { 480.0 };
    let ar = if art.w > 0.0 && art.h > 0.0 { (art.w / art.h) as f32 } else { 4.0 / 3.0 };
    let body_h = (card_w / ar).clamp(80.0, 560.0);
    view()
        .mt_1()
        .w(px(card_w))
        .bg(theme::CARD)
        .rounded(px(12.0))
        .border_1()
        .border_color(theme::BORDER_F)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .pt_3()
                .child(icon("box"))
                .child(
                    text(if art.title.is_empty() { art.app.clone() } else { art.title.clone() })
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::TEXT)
                        .flex_1()
                        .number_of_lines(1),
                ),
        )
        .child(
            // Body: the live mini app (compiled TSX running in its own
            // engine inside this element), or the load/compile error.
            view()
                .h(px(body_h))
                .m_2()
                .rounded(px(8.0))
                .child(crate::mini::mini_app(&art.app, body_h - 4.0)),
        )
        .child(when(
            {
                let has = !art.desc.is_empty();
                move || has
            },
            {
                let d = art.desc.clone();
                move || {
                    text(d.clone())
                        .text_size(px(11.0))
                        .text_color(theme::MUTED)
                        .mx_3()
                        .mb_1()
                        .into_element()
                }
            },
        ))
        .child({
            let err_app = art.app.clone();
            let err_app2 = art.app.clone();
            view().mx_2().mb_1().child(when(
                move || !store().mini_err(&err_app).get().is_empty(),
                move || {
                    let err = store().mini_err(&err_app2).get();
                    text(format!("widget: {err}"))
                        .text_size(px(10.0))
                        .text_color(theme::DOWN)
                        .number_of_lines(2)
                        .into_element()
                },
            ))
        })
        .child(
            view()
                .flex_row()
                .gap_1()
                .px_2()
                .pb_2()
                .child(card_action("panel", "Details", move || {
                    let st = store();
                    st.load_detail(app.clone());
                    st.right_open.set(true);
                }))
                .child(card_action("pin", "Pin", move || {
                    let st = store();
                    let thread = st.active_id.get_untracked();
                    crate::client::post_bg(
                        "/thread/apps".to_owned(),
                        serde_json::json!({"thread": thread, "app": app2}),
                    );
                    st.refresh_thread_apps();
                }))
                .child({
                    let pop = art.app.clone();
                    card_action("popout", "Pop out", move || store().popout(&pop))
                })
                .child({
                    let ret = art.app.clone();
                    let ret2 = art.app.clone();
                    when(
                        move || !store().mini_err(&ret).get().is_empty(),
                        move || {
                            let r = ret2.clone();
                            card_action("retry", "Retry", move || store().retry_mini(&r))
                                .into_element()
                        },
                    )
                }),
        )
}

fn card_action(
    name: &'static str,
    label: &'static str,
    on: impl Fn() + 'static,
) -> impl IntoElement {
    let hovered = signal(false);
    view()
        .label(label)
        .child(
            view()
                .size(px(28.0))
                .rounded(px(7.0))
                .items_center()
                .justify_center()
                .bg(move || if hovered.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
                .child(icon(name)),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || on())
}
