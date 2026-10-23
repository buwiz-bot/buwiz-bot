//! The "Create New Bot" sheet (`botModal`, App.tsx:4229): a centered
//! 440pt dialog — live 96pt `BotBlob` preview, name field, the
//! `BOT_SHAPES` grid in the picked ink, the `BOT_COLORS` swatch row, and
//! Cancel / Create pills. `botHover` marks the hovered control.

use std::str::FromStr;

use nsh_core::ui::prelude::*;

use crate::avatar::bot_avatar;
use crate::icons::icon;
use crate::live::store;
use crate::theme;

/// The overlay layer drawn while `bot_modal` is open.
pub(crate) fn layer() -> impl IntoElement {
    dynamic(|| {
        if !store().bot_modal.get() {
            return view().into_element();
        }
        sheet().into_element()
    })
}

fn sheet() -> impl IntoElement {
    view()
        .size_full()
        .absolute()
        .items_center()
        .justify_center()
        .child(
            // Scrim behind the sheet (sheet taps never reach it).
            view()
                .size_full()
                .absolute()
                .bg(hsla(0.0, 0.0, 0.0, 0.5))
                .on_press(|| store().bot_modal.set(false)),
        )
        .child(
            view()
                .w(px(440.0))
                .rounded(px(14.0))
                .bg(theme::MENU_BG)
                .border_1()
                .border_color(theme::BORDER_S)
                .child(header())
                .child(body())
                .child(footer()),
        )
}

fn header() -> impl IntoElement {
    let hov = signal(false);
    view()
        .h(px(55.0))
        .flex_row()
        .items_center()
        .pl_4()
        .pr(px(10.0))
        .child(
            text("Create New Bot")
                .flex_1()
                .text_size(px(14.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::TEXT),
        )
        .child(
            view()
                .size(px(32.0))
                .rounded(px(16.0))
                .items_center()
                .justify_center()
                .bg(move || if hov.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
                .child(icon("close"))
                .on_hover_in(move || hov.set(true))
                .on_hover_out(move || hov.set(false))
                .on_press(|| store().bot_modal.set(false))
                .label("Close dialog"),
        )
}

fn body() -> impl IntoElement {
    view()
        .px_4()
        .py_4()
        .items_center()
        .child(
            // The 96pt live preview: re-mounts when the pick changes so
            // the blob takes the new shape / ink.
            dynamic(|| {
                let st = store();
                let shape = st.bot_shape.get();
                let color = crate::mark::ink_of(&st.bot_color.get(), !st.dark.get());
                view()
                    .size(px(96.0))
                    .items_center()
                    .justify_center()
                    .child(bot_avatar(crate::avatar::BotAvatarProps {
                        shape,
                        color,
                        size: 96.0,
                        ..Default::default()
                    }))
                    .into_element()
            }),
        )
        .child(
            view()
                .w_full()
                .mt_4()
                .h(px(35.0))
                .rounded(px(8.0))
                .border_1()
                .border_color(theme::BORDER_S)
                .justify_center()
                .px(px(10.0))
                .child(
                    input()
                        .value(move || store().bot_name.get())
                        .placeholder("Name your Bot")
                        .placeholder_color(|| color(theme::GHOST))
                        .text_size(px(13.0))
                        .text_color(theme::TEXT)
                        .on_change(move |v: &str| store().bot_name.set(v.to_owned()))
                        .on_submit(|_: &str| store().create_bot())
                        .h(px(30.0)),
                ),
        )
        .child(dynamic(shapes))
        .child(dynamic(colors))
}

/// The shape grid (`BOT_SHAPES`): 48pt cells in the picked ink; the
/// ring marks the pick, a fainter one the hover.
fn shapes() -> impl IntoElement {
    let color = {
        let st = store();
        crate::mark::ink_of(&st.bot_color.get(), !st.dark.get())
    };
    view()
        .w(px(272.0))
        .mt_4()
        .flex_row()
        .flex_wrap()
        .justify_center()
        .gap_2()
        .child(each(
            || crate::mark::BOT_SHAPES.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            |s| s.clone(),
            move |s, _| {
                let sh = s.get();
                let sh2 = sh.clone();
                let sh3 = sh.clone();
                let sh4 = sh.clone();
                let key = format!("s:{sh}");
                let key2 = key.clone();
                let color = color.clone();
                view()
                    .size(px(48.0))
                    .items_center()
                    .justify_center()
                    .child(
                        view()
                            .absolute()
                            .size(px(44.0))
                            .rounded(px(22.0))
                            .border_1()
                            .border_color(move || {
                                let st = store();
                                if st.bot_shape.get() == sh {
                                    theme::BORDER_S
                                } else if st.bot_hover.get() == key {
                                    theme::BORDER_F
                                } else {
                                    AdaptiveColor::TRANSPARENT
                                }
                            }),
                    )
                    .child(bot_avatar(crate::avatar::BotAvatarProps {
                        shape: sh2,
                        color,
                        size: 44.0,
                        paused: true,
                        ..Default::default()
                    }))
                    .on_hover_in(move || store().bot_hover.set(key2.clone()))
                    .on_hover_out(move || {
                        let st = store();
                        if st.bot_hover.get_untracked().starts_with("s:") {
                            st.bot_hover.set(String::new());
                        }
                    })
                    .on_press(move || store().bot_shape.set(sh3.clone()))
                    .label(format!("{sh4} character shape"))
            },
        ))
}

/// The swatch row (`BOT_COLORS`): 32pt buttons, 24pt dots, a 1pt ring
/// on the pick.
fn colors() -> impl IntoElement {
    view()
        .w(px(232.0))
        .mt_3()
        .flex_row()
        .flex_wrap()
        .justify_center()
        .gap_2()
        .child(each(
            || crate::mark::BOT_COLORS.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            |s| s.clone(),
            move |c, _| {
                let hex = c.get();
                let hex2 = hex.clone();
                let hex3 = hex.clone();
                let ink = crate::mark::ink_of(&hex, false);
                view()
                    .size(px(32.0))
                    .rounded(px(16.0))
                    .items_center()
                    .justify_center()
                    .border_1()
                    .border_color(move || {
                        if store().bot_color.get() == hex {
                            theme::BORDER_S
                        } else {
                            AdaptiveColor::TRANSPARENT
                        }
                    })
                    .child(
                        view()
                            .size(px(24.0))
                            .rounded(px(12.0))
                            .bg(Hsla::from_str(&ink).unwrap_or(Hsla { h: 0.0, s: 0.0, l: 0.0, a: 1.0 })),
                    )
                    .on_press(move || store().bot_color.set(hex2.clone()))
                    .label(format!("{hex3} character color"))
            },
        ))
}

fn footer() -> impl IntoElement {
    let hov_cancel = signal(false);
    view()
        .flex_row()
        .justify_end()
        .gap_2()
        .py_3()
        .pl_4()
        .pr_3()
        .border_t_1()
        .border_t_color(theme::BORDER_S)
        .child(
            view()
                .h(px(32.0))
                .rounded(px(16.0))
                .px_3()
                .justify_center()
                .bg(move || {
                    if hov_cancel.get() {
                        theme::HOVER
                    } else {
                        theme::SUBTLE
                    }
                })
                .child(text("Cancel").text_size(px(14.0)).text_color(theme::TEXT))
                .on_hover_in(move || hov_cancel.set(true))
                .on_hover_out(move || hov_cancel.set(false))
                .on_press(|| store().bot_modal.set(false)),
        )
        .child(
            view()
                .h(px(32.0))
                .rounded(px(16.0))
                .px_3()
                .justify_center()
                .bg(move || {
                    if store().bot_name.get().trim().is_empty() {
                        theme::SUBTLE
                    } else {
                        theme::TEXT
                    }
                })
                .child(
                    text("Create")
                        .text_size(px(14.0))
                        .text_color(move || {
                            if store().bot_name.get().trim().is_empty() {
                                theme::GHOST
                            } else {
                                theme::MENU_BG
                            }
                        }),
                )
                .on_press(|| store().create_bot()),
        )
}
