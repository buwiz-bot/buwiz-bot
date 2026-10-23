//! The right detail pane (`fileviewer.tsx` + thread apps + app state):
//! shows `/state/:app` entries and the apps pinned to the open thread.

use nsh_core::ui::prelude::*;

use crate::icons::icon;
use crate::live::store;
use crate::theme;

/// The right column — opened by a bot row or an app card.
pub(crate) fn pane() -> impl IntoElement {
    let st = store();
    view()
        .w(px(280.0))
        .h_full()
        .bg(theme::PANE)
        .border_l_1()
        .border_l_color(theme::BORDER_F)
        .child(
            view()
                .flex_row()
                .items_center()
                .px_3()
                .pt(px(46.0))
                .pb_2()
                .child(
                    text(move || {
                        let app = st.detail_app.get();
                        if app.is_empty() { "Details".to_owned() } else { app }
                    })
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT)
                    .flex_1()
                    .number_of_lines(1),
                )
                .child(
                    view()
                        .label("Close")
                        .child(icon("close"))
                        .on_press(|| store().right_open.set(false)),
                ),
        )
        .child(
            scroll().flex_1().child(
                view()
                    .px_3()
                    .pb_4()
                    .child(section("Pinned apps"))
                    .child(each(
                        || store().thread_apps.get(),
                        |a| a.app.clone(),
                        |a, _| pinned_row(a.get()),
                    ))
                    .child(section("State"))
                    .child(each(
                        || store().detail.get(),
                        |kv| kv.0.clone(),
                        |kv, _| kv_row(kv.get()),
                    )),
            ),
        )
}

fn section(label: &'static str) -> impl IntoElement {
    text(label)
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::GHOST)
        .pt(px(12.0))
        .pb(px(4.0))
}

fn pinned_row(a: crate::store::ThreadApp) -> impl IntoElement {
    let hovered = signal(false);
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_2()
        .py(px(6.0))
        .rounded(px(7.0))
        .bg(move || if hovered.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
        .child(icon("box"))
        .child(
            text(if a.title.is_empty() { a.app.clone() } else { a.title.clone() })
                .text_size(px(12.0))
                .text_color(theme::TEXT)
                .number_of_lines(1),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
}

fn kv_row(kv: crate::store::Kv) -> impl IntoElement {
    view()
        .py(px(5.0))
        .child(
            text(kv.0)
                .text_size(px(11.0))
                .font_family("monospace")
                .text_color(theme::MUTED)
                .number_of_lines(1),
        )
        .child(
            text(kv.1)
                .text_size(px(12.0))
                .font_family("monospace")
                .text_color(theme::TEXT)
                .number_of_lines(3),
        )
}
