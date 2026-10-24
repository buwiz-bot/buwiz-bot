//! The ⌘K palette (`palette.tsx`): a centered card with a search field
//! listing the shell's commands, the agents (bot search) and the
//! published mini apps; arrows/enter drive `palette_sel`, Escape or the
//! scrim dismisses.

use nsh_core::ui::prelude::*;

use crate::icons::icon_sized;
use crate::live::store;
use crate::store::PaletteItem;
use crate::theme;

/// The overlay layer drawn while `palette` is open.
pub(crate) fn layer() -> impl IntoElement {
    dynamic(|| {
        if !store().palette.get() {
            return view().into_element();
        }
        palette_card().into_element()
    })
}

fn palette_card() -> impl IntoElement {
    let h = window().height.0;
    let card = view()
        .w(px(486.0))
        .max_h(px(395.0))
        .bg(theme::CARD2)
        .rounded(px(14.0))
        .border(px(0.5))
        .border_color(theme::BORDER_F)
        .overflow_hidden()
        .child(
            // The search row — `⌕` + autofocused Input (palette.tsx:72).
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(44.0))
                .px(px(14.0))
                .border_b(px(0.5))
                .border_b_color(theme::BORDER_F)
                .child(
                    text("⌕")
                        .text_size(px(13.0))
                        .text_color(theme::GHOST),
                )
                .child(
                    input()
                        .value(move || store().palette_q.get())
                        .on_change(|v: &str| {
                            let st = store();
                            st.palette_q.set(v.to_owned());
                            st.palette_sel.set(0);
                        })
                        .on_submit(|_: &str| {
                            let st = store();
                            st.palette_run_at(st.palette_sel.get_untracked() as usize);
                        })
                        .auto_focus(true)
                        .placeholder("Search")
                        .placeholder_color(|| color(theme::GHOST))
                        .text_size(px(14.0))
                        .text_color(theme::TEXT)
                        .flex_1(),
                ),
        )
        .child(
            scroll().max_h(px(395.0 - 44.0)).child(
                view()
                    .p(px(6.0))
                    .child(each(
                        || store().palette_results(),
                        |i| (i.title.clone(), i.subtitle.clone(), i.icon.clone()),
                        |it, ix| palette_row(it, ix),
                    ))
                    .child(when(
                        || store().palette_results().is_empty(),
                        || {
                            text("No results")
                                .text_size(px(13.0))
                                .text_color(theme::GHOST)
                                .p(px(12.0))
                                .into_element()
                        },
                    )),
            ),
        );
    view()
        .size_full()
        .absolute()
        .items_center()
        // The transparent dismiss layer (palette.tsx:67) — absolute so it
        // doesn't take column space above the card.
        .child(
            view()
                .size_full()
                .absolute()
                .on_press(|| store().close_palette()),
        )
        .child(card.mt(px(((h - 395.0) / 2.0).max(40.0))))
}

fn palette_row(it: ReadSignal<PaletteItem>, ix: ReadSignal<usize>) -> impl IntoElement {
    let sel = move || store().palette_sel.get() as usize == ix.get();
    view()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .h(px(44.0))
                .px_2()
                .rounded(px(8.0))
                .bg(move || {
                    if sel() {
                        theme::HOVER
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .child(
                    // The 20×20 tile — an icon, or the agent's letter
                    // (palette.tsx:105).
                    view()
                        .size(px(20.0))
                        .rounded(px(5.0))
                        .items_center()
                        .justify_center()
                        .bg(theme::SUBTLE)
                        .child(dynamic(move || {
                            let icon_name = it.get().icon;
                            if icon_name.chars().count() == 1 {
                                text(icon_name)
                                    .text_size(px(11.0))
                                    .text_color(theme::MUTED)
                                    .into_element()
                            } else {
                                let name: &'static str = match icon_name.as_str() {
                                    "newChat" => "newChat",
                                    "systemTheme" => "systemTheme",
                                    "sun" => "sun",
                                    "moon" => "moon",
                                    _ => "gpts",
                                };
                                icon_sized(name, 13.0).into_element()
                            }
                        })),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(
                            text(move || it.get().title)
                                .text_size(px(13.0))
                                .line_height(px(18.0))
                                .text_color(theme::TEXT)
                                .number_of_lines(1),
                        )
                        .child(when(
                            move || !it.get().subtitle.is_empty(),
                            move || {
                                text(move || it.get().subtitle)
                                    .text_size(px(12.0))
                                    .line_height(px(16.0))
                                    .text_color(theme::MUTED)
                                    .number_of_lines(1)
                                    .into_element()
                            },
                        )),
                )
                .child(when(
                    move || it.get().checked,
                    || icon_sized("check", 13.0).into_element(),
                ))
                .child(when(
                    move || {
                        !store().palette_q.get().trim().is_empty() && ix.get() < 9
                    },
                    move || {
                        text(move || format!("⌘{}", ix.get() + 1))
                            .text_size(px(11.0))
                            .text_color(theme::GHOST)
                            .into_element()
                    },
                )),
        )
        .on_hover_in(move || store().palette_sel.set(ix.get() as i64))
        .on_press(move || store().palette_run_at(ix.get()))
        .into_element()
}

