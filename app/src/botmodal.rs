//! The "Create New Bot" sheet (`botModal`, App.tsx:4229): a centered
//! 440pt dialog — live 96pt `BotBlob` preview, name field, the
//! `BOT_SHAPES` grid in the picked ink, the `BOT_COLORS` swatch row, and
//! Cancel / Create pills. `botHover` marks the hovered control.

use std::str::FromStr;

use nsh_base::input;
use nsh_core::prelude::*;

use crate::avatar::bot_avatar;
use crate::icons::icon;
use crate::links::LinkAsk;
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
                .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
                .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
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
                // Raw swatch — `BotBlob`'s painter resolves `inkOf` per
                // theme itself (blob.tsx:70).
                let color = st.bot_color.get();
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
                        // Type straight away: the modal opens on its field.
                        .auto_focus(true)
                        // A focused field takes Escape for itself (it would
                        // only leave the field): close the modal from here.
                        .on_key_press(|key: &str| {
                            if key == "Escape" {
                                store().bot_modal.set(false);
                            }
                        })
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
    let color = store().bot_color.get();
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
                    .on_mouse_enter(move |_: &MouseEvent| store().bot_hover.set(key2.clone()))
                    .on_mouse_leave(move |_: &MouseEvent| {
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
                let hex4 = hex.clone();
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
                            // `#FFFFFF` is the primary swatch: black on light, white
                            // on dark, as the avatar paints it (`ink_of`).
                            .bg(move || {
                                let ink = crate::mark::ink_of(&hex4, !dark());
                                Hsla::from_str(&ink).unwrap_or(Hsla { h: 0.0, s: 0.0, l: 0.0, a: 1.0 })
                            }),
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
                .on_mouse_enter(move |_: &MouseEvent| hov_cancel.set(true))
                .on_mouse_leave(move |_: &MouseEvent| hov_cancel.set(false))
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

/// "Choose a primary Bot" (Grok Bot's): the agents with a search field;
/// the pick is checked, Confirm makes it the primary Bot.
pub(crate) fn main_picker_layer() -> impl IntoElement {
    dynamic(|| {
        if store().main_pick.get().is_none() {
            return view().into_element();
        }
        let close = || store().main_pick.set(None);
        view()
            .size_full()
            .absolute()
            .items_center()
            .justify_center()
            .bg(theme::tone(theme::hsla(0.0, 0.0, 0.0, 0.25), theme::hsla(0.0, 0.0, 0.0, 0.5)))
            .on_press(close)
            .child(
                view()
                    .w(px(420.0))
                    .rounded(px(16.0))
                    .bg(theme::POPOVER)
                    .border_1()
                    .border_color(theme::BORDER_F)
                    .overflow_hidden()
                    // A press inside the card stays in it.
                    .on_press(|| {})
                    .child(
                        text("Choose a primary Bot")
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT)
                            .px_4()
                            .py_3(),
                    )
                    .child(
                        view().px_4().pb_2().child(
                            input()
                                .value(|| store().main_pick_q.get())
                                .on_change(|v: &str| store().main_pick_q.set(v.to_owned()))
                                .on_key_press(|k: &str| {
                                    if k == "Escape" {
                                        store().main_pick.set(None);
                                    }
                                })
                                .auto_focus(true)
                                .placeholder("Search")
                                .placeholder_color(|| color(theme::GHOST))
                                .text_size(px(13.0))
                                .text_color(theme::TEXT)
                                .h(px(34.0))
                                .px(px(10.0))
                                .rounded(px(8.0))
                                .border_1()
                                .border_color(theme::BORDER_S),
                        ),
                    )
                    .child(
                        scroll().max_h(px(320.0)).child(view().px_2().pb_2().child(each(
                            || {
                                let q = store().main_pick_q.get().trim().to_lowercase();
                                store()
                                    .bots
                                    .get()
                                    .into_iter()
                                    .filter(|b| q.is_empty() || b.name.to_lowercase().contains(&q))
                                    .collect::<Vec<_>>()
                            },
                            |b| b.id.clone(),
                            |b, _| {
                                let id = b.get_untracked().id;
                                let id2 = id.clone();
                                let id3 = id.clone();
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap_3()
                                    .h(px(44.0))
                                    .px_2()
                                    .rounded(px(8.0))
                                    .bg(move || {
                                        if store().main_pick.get().as_deref() == Some(id.as_str()) {
                                            theme::HOVER
                                        } else {
                                            AdaptiveColor::TRANSPARENT
                                        }
                                    })
                                    .on_press(move || store().main_pick.set(Some(id2.clone())))
                                    .child(dynamic(move || {
                                        bot_avatar(crate::avatar::BotAvatarProps {
                                            box_: Some(26.0),
                                            paused: true,
                                            ..crate::avatar::BotAvatarProps::bot(&b.get(), 30.0)
                                        })
                                        .into_element()
                                    }))
                                    .child(
                                        text(move || b.get().name)
                                            .text_size(px(14.0))
                                            .text_color(theme::TEXT)
                                            .flex_1()
                                            .number_of_lines(1),
                                    )
                                    .child(when(
                                        move || store().main_pick.get().as_deref() == Some(id3.as_str()),
                                        || text("✓").text_size(px(14.0)).text_color(theme::TEXT).into_element(),
                                    ))
                                    .into_element()
                            },
                        ))),
                    )
                    .child(
                        view()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .px_4()
                            .py_3()
                            .border_t_1()
                            .border_t_color(theme::BORDER_S)
                            .child(dialog_button("Cancel", false, close))
                            .child(dialog_button("Confirm", true, || {
                                let st = store();
                                if let Some(id) = st.main_pick.get_untracked() {
                                    st.set_main_bot(&id);
                                }
                                st.main_pick.set(None);
                            })),
                    ),
            )
            .into_element()
    })
}

/// A link waiting on the person (`Store::link_ask`), in the same dialog
/// as the primary-Bot picker: an outside URL of another app's scheme
/// (Open), a new chat another app asked for (Start chat), or a plugin's
/// mini app another app or a tool result asked for (Open). It names what
/// it would do; Cancel, Escape or a press outside drops it.
pub(crate) fn link_ask_layer() -> impl IntoElement {
    dynamic(|| {
        let Some(ask) = store().link_ask.get() else {
            return view().into_element();
        };
        let (head, what, body, yes) = match ask {
            LinkAsk::Open(url) => (
                "Open this link in another app?".to_owned(),
                "A link in Rox wants to open outside it:",
                url,
                "Open",
            ),
            LinkAsk::NewChat(draft) => (
                "Start a new chat?".to_owned(),
                "Another app asked Rox to start a chat with this draft. Nothing is sent until you send it.",
                if draft.trim().is_empty() { "(no draft)".to_owned() } else { draft },
                "Start chat",
            ),
            LinkAsk::App(app) => (
                store().mcp_manifest.with(|ms| crate::links::app_question(&app, ms)),
                match app.from {
                    crate::links::AppOpener::ToolResult => "A tool's result asked Rox to show this app full screen:",
                    _ => "Another app or a web page asked Rox to show this app full screen:",
                },
                crate::links::app_subject(&app).unwrap_or(app.key.as_str()).to_owned(),
                "Open",
            ),
        };
        let close = || store().answer_link_ask(false);
        view()
            .size_full()
            .absolute()
            .items_center()
            .justify_center()
            .bg(theme::tone(theme::hsla(0.0, 0.0, 0.0, 0.25), theme::hsla(0.0, 0.0, 0.0, 0.5)))
            .on_press(close)
            .child(
                view()
                    .w(px(420.0))
                    .rounded(px(16.0))
                    .bg(theme::POPOVER)
                    .border_1()
                    .border_color(theme::BORDER_F)
                    .overflow_hidden()
                    .label(head.clone())
                    // A press inside the card stays in it.
                    .on_press(|| {})
                    .child(
                        text(head)
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT)
                            .px_4()
                            .pt_3()
                            .pb_1(),
                    )
                    .child(text(what).text_size(px(12.0)).text_color(theme::MUTED).px_4().pb_2())
                    .child(
                        view().px_4().pb_3().child(
                            text(body)
                                .number_of_lines(6)
                                .text_size(px(13.0))
                                .text_color(theme::TEXT)
                                .px(px(10.0))
                                .py(px(8.0))
                                .rounded(px(8.0))
                                .bg(theme::SUBTLE),
                        ),
                    )
                    .child(
                        view()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .px_4()
                            .py_3()
                            .border_t_1()
                            .border_t_color(theme::BORDER_S)
                            .child(dialog_button("Cancel", false, close))
                            .child(dialog_button(yes, true, || store().answer_link_ask(true))),
                    ),
            )
            .into_element()
    })
}

fn dialog_button(label: &'static str, primary: bool, on: impl Fn() + 'static) -> impl IntoElement {
    view()
        .h(px(30.0))
        .px_3()
        .rounded(px(15.0))
        .items_center()
        .justify_center()
        .bg(if primary { theme::TEXT } else { theme::SUBTLE })
        .child(
            text(label)
                .text_size(px(13.0))
                .text_color(if primary { theme::POPOVER } else { theme::TEXT }),
        )
        .on_press(on)
}
