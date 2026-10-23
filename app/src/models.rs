//! The model picker (`modelpicker.tsx`): a card listing `/models`; the
//! current one is checked, choosing posts `/prefs` and closes.

use nsh_core::ui::prelude::*;

use crate::icons::icon;
use crate::live::store;
use crate::store::ModelDef;
use crate::theme;

/// The overlay layer drawn while `models_open` is set.
pub(crate) fn layer() -> impl IntoElement {
    dynamic(|| {
        if !store().models_open.get() {
            return view().into_element();
        }
        card().into_element()
    })
}

fn card() -> impl IntoElement {
    view()
        .size_full()
        .absolute()
        .child(view().size_full().on_press(|| store().models_open.set(false)))
        .child(
            view()
                .absolute()
                .bottom(px(96.0))
                .left(px(24.0))
                .w(px(300.0))
                .bg(theme::MENU_BG)
                .rounded(px(12.0))
                .border_1()
                .border_color(theme::BORDER_S)
                .child(
                    text("Model")
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::GHOST)
                        .px_3()
                        .pt(px(10.0))
                        .pb(px(4.0)),
                )
                .child(
                    scroll()
                        .max_h(px(320.0))
                        .child(each(
                            || store().models.get(),
                            |m| m.id.clone(),
                            |m, _| model_row(m.get()),
                        )),
                )
                .child(effort_section())
                .child(fast_row()),
        )
}

/// The effort choices of the selected model, under the list — the TSX
/// slider's steps as rows (a canvas slider can replace them).
fn effort_section() -> impl IntoElement {
    dynamic(|| {
        let st = store();
        let id = st.model.get();
        let Some(m) = st.models.get_untracked().iter().find(|m| m.id == id).cloned() else {
            return view().into_element();
        };
        if m.efforts.len() < 2 {
            return view().into_element();
        }
        let efforts = m.efforts.clone();
        view()
            .border_t_1()
            .border_t_color(theme::BORDER_F)
            .child(
                text("Effort")
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::GHOST)
                    .px_3()
                    .pt(px(8.0))
                    .pb(px(2.0)),
            )
            .child(each(
                move || efforts.clone(),
                |e| e.0.clone(),
                |e_sig, _| effort_row(e_sig.get()),
            ))
            .pb_1()
            .into_element()
    })
}

fn effort_row(e: (String, String)) -> impl IntoElement {
    let value = e.0.clone();
    let current = move || store().effort.get() == e.0;
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .py(px(6.0))
        .child(
            text(e.1)
                .text_size(px(12.0))
                .text_color(theme::TEXT)
                .flex_1(),
        )
        .child(when(current, || icon("check").into_element()))
        .on_press(move || store().choose_effort(value.clone()))
}

fn fast_row() -> impl IntoElement {
    dynamic(|| {
        let st = store();
        let id = st.model.get();
        let Some(m) = st.models.get_untracked().iter().find(|m| m.id == id).cloned() else {
            return view().into_element();
        };
        if !m.fast {
            return view().into_element();
        }
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py(px(8.0))
            .border_t_1()
            .border_t_color(theme::BORDER_F)
            .child(icon("bolt"))
            .child(
                text("Fast mode")
                    .text_size(px(12.0))
                    .text_color(theme::TEXT)
                    .flex_1(),
            )
            .child(when(
                move || store().fast.get(),
                || text("on").text_size(px(11.0)).text_color(theme::ACCENT).into_element(),
            ))
            .on_press(|| store().toggle_fast())
            .into_element()
    })
}

fn model_row(m: ModelDef) -> impl IntoElement {
    let hovered = signal(false);
    let id = m.id.clone();
    let current = move || store().model.get() == m.id;
    view()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py(px(8.0))
                .bg(move || if hovered.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
                .child(
                    view()
                        .flex_1()
                        .child(
                            text(m.label)
                                .text_size(px(13.0))
                                .text_color(theme::TEXT)
                                .number_of_lines(1),
                        )
                        .when(!m.provider.is_empty(), |v| {
                            v.child(
                                text(m.provider)
                                    .text_size(px(11.0))
                                    .text_color(theme::GHOST)
                                    .number_of_lines(1),
                            )
                        }),
                )
                .child(when(current, || icon("check").into_element())),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || {
            let st = store();
            st.choose_model(id.clone());
            st.models_open.set(false);
        })
}
