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
    view()
        .mt_1()
        .w(px(340.0))
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
            // Body: where the generated TSX ran; a Rust build shows the
            // placeholder and offers the detail pane instead.
            view()
                .h(px(120.0))
                .m_2()
                .bg(theme::CARD2)
                .rounded(px(8.0))
                .items_center()
                .justify_center()
                .child(
                    text("Generated app")
                        .text_size(px(12.0))
                        .text_color(theme::GHOST),
                )
                .child(
                    text(app.clone())
                        .text_size(px(10.0))
                        .font_family("monospace")
                        .text_color(theme::GHOST)
                        .pt_1(),
                ),
        )
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
                })),
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
