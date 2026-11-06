//! The avatar Design panel: a prompt, an optional picture, and the 3 mark
//! avatars the broker designs from them (`POST /avatar/design`), on the
//! Create New Bot sheet. "Your designs" is on the sheet and Edit Avatar's Own
//! tab; `for_new` says where a tap lands (the sheet's draft, or the bot).

use nsh_base::input;
use nsh_core::prelude::*;

use crate::avatar::{bot_blob, cropped_image, BotBlobProps};
use crate::live::store;
use crate::theme;

pub(crate) fn panel() -> impl IntoElement {
    view()
        .w_full()
        .gap(px(10.0))
        .child(mode_switch())
        .child(prompt_field())
        .child(dynamic(|| picture_row().into_element()))
        .child(design_button())
        .child(
            text(|| format!("with {}", store().design_model().1))
                .text_size(px(12.0))
                .text_color(theme::MUTED),
        )
        .child(dynamic(|| choices().into_element()))
        .child(error_line())
}

/// Mark (3 one-colour avatars that move) | Rich (3 multi-colour ones,
/// their colours their own).
fn mode_switch() -> impl IntoElement {
    let modes: &[(&'static str, &'static str)] = &[("mark", "Mark"), ("rich", "Rich")];
    view()
        .flex_row()
        .gap(px(4.0))
        .children(modes.iter().copied().map(|(m, label)| {
            view()
                .h(px(24.0))
                .px(px(10.0))
                .rounded(px(12.0))
                .justify_center()
                .bg(move || if store().design_mode.get() == m { theme::ACTIVE } else { theme::SUBTLE })
                .child(
                    text(label)
                        .text_size(px(12.0))
                        .text_color(move || if store().design_mode.get() == m { theme::TEXT } else { theme::DIM }),
                )
                .label(format!("{label} mode"))
                .on_press(move || {
                    let st = store();
                    st.design_mode.set(m);
                    st.design_err.set(String::new());
                })
                .into_element()
        }))
}

fn prompt_field() -> impl IntoElement {
    input()
        .value(|| store().design_prompt.get())
        .on_change(|v: &str| store().design_prompt.set(v.to_owned()))
        .placeholder(|| "Describe the bot, or add a picture and leave this empty".to_owned())
        .placeholder_color(|| color(theme::GHOST))
        .multiline(|| true)
        .readonly(|| store().design_busy.get())
        .h(px(72.0))
        .text_size(px(13.0))
        .text_color(theme::TEXT)
        .bg(theme::CARD2)
        .py_2()
        .px(px(10.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(theme::BORDER_S)
}

/// The attached picture (thumbnail, name, remove), or the buttons that
/// attach one.
fn picture_row() -> impl IntoElement {
    match store().design_image.get() {
        Some(d) => view()
            .flex_row()
            .items_center()
            .gap_2()
            .child(cropped_image(&d.uri, 28.0, None))
            .child(
                text(d.name.clone())
                    .flex_1()
                    .text_size(px(12.0))
                    .text_color(theme::DIM),
            )
            .child(
                view()
                    .size(px(22.0))
                    .rounded(px(11.0))
                    .items_center()
                    .justify_center()
                    .child(text("✕").text_size(px(12.0)).text_color(theme::DIM))
                    .label("Remove the picture")
                    .on_press(|| store().design_image.set(None)),
            )
            .into_element(),
        None => view()
            .flex_row()
            .items_center()
            .gap_2()
            .child(pill("Add picture", "Add a picture to design from", || store().design_attach("pick")))
            .child(pill("Paste", "Paste a picture to design from", || store().design_attach("paste")))
            .child(text("optional").text_size(px(12.0)).text_color(theme::GHOST))
            .into_element(),
    }
}

fn pill(label: &'static str, a11y: &'static str, on: impl Fn() + 'static) -> impl IntoElement {
    view()
        .h(px(24.0))
        .px(px(10.0))
        .rounded(px(12.0))
        .bg(theme::SUBTLE)
        .justify_center()
        .child(text(label).text_size(px(12.0)).text_color(theme::TEXT))
        .label(a11y)
        .on_press(on)
}

/// Ready when there is a prompt or a picture and no design in flight.
fn ready() -> bool {
    let st = store();
    !st.design_busy.get() && (!st.design_prompt.get().trim().is_empty() || st.design_image.get().is_some())
}

fn design_button() -> impl IntoElement {
    view()
        .h(px(30.0))
        .rounded(px(10.0))
        .items_center()
        .justify_center()
        .bg(|| if ready() { theme::TEXT } else { theme::SUBTLE })
        .child(
            text(|| if store().design_busy.get() { "Designing…" } else { "Design" })
                .text_size(px(13.0))
                .text_color(|| if ready() { theme::MENU_BG } else { theme::GHOST }),
        )
        .label("Design avatars")
        .on_press(|| store().design_avatar())
}

/// The colour the options are drawn in: the sheet's draft colour, or the
/// open bot's (the primary ink when it wears a photo).
fn current_color(for_new: bool) -> String {
    let st = store();
    if for_new {
        return st.draft_accent();
    }
    st.active_bot()
        .filter(|b| b.image.is_empty())
        .map(|b| b.accent())
        .unwrap_or_else(|| "#FFFFFF".to_owned())
}

/// The 3 choices, live; the picked one is outlined and named below. A
/// choice in the primary ink shows in the draft's colour, which it keeps.
fn choices() -> impl IntoElement {
    let st = store();
    let cands = st.design_cands.get();
    if cands.is_empty() {
        return view().into_element();
    }
    let ink = current_color(true);
    let picked = st.design_picked.get();
    let caption = match picked.and_then(|i| cands.get(i)) {
        Some(c) if c.why.is_empty() => c.name.clone(),
        Some(c) => format!("{} · {}", c.name, c.why),
        None => "Tap one to use it".to_owned(),
    };
    view()
        .gap(px(6.0))
        .child(
            view()
                .flex_row()
                .justify_center()
                .gap_2()
                .children(cands.into_iter().enumerate().map(|(i, c)| {
                    view()
                        .size(px(76.0))
                        .rounded(px(14.0))
                        .items_center()
                        .justify_center()
                        .border_1()
                        .border_color(if picked == Some(i) { theme::TEXT } else { theme::BORDER_S })
                        .label(format!("Use the {} avatar", c.name))
                        .on_press(move || store().design_pick(i))
                        .child(bot_blob(BotBlobProps {
                            shape: c.shape,
                            color: if crate::store::is_primary_ink(&c.color) { ink.clone() } else { c.color },
                            size: 58.0,
                            seed: Some(format!("design{i}")),
                            ..Default::default()
                        }))
                        .into_element()
                })),
        )
        .child(text(caption).text_size(px(12.0)).text_color(theme::DIM))
        .into_element()
}

fn error_line() -> impl IntoElement {
    when(
        || !store().design_err.get().is_empty(),
        || {
            text(|| store().design_err.get())
                .text_size(px(12.0))
                .text_color(theme::DOWN)
                .into_element()
        },
    )
}

/// "Your designs": the shape of every Design pick, newest first, at rest
/// and in the bot's colour (like the kit grid). A tap puts the shape on
/// the bot (or the sheet's draft), which keeps its colour; hover shows ✕
/// to remove it. Nothing is drawn while the list is empty.
pub(crate) fn saved(for_new: bool) -> impl IntoElement {
    dynamic(move || {
        let st = store();
        let saved = st.design_saved.get();
        if saved.is_empty() {
            return view().into_element();
        }
        let current = if for_new {
            Some(st.bot_shape.get())
        } else {
            st.active_bot().filter(|b| b.image.is_empty()).map(|b| b.shape)
        };
        let ink = current_color(for_new);
        let removing = st
            .design_removing
            .get()
            .and_then(|i| saved.get(i).map(|c| (i, c.name.clone())));
        view()
            .w_full()
            .gap(px(8.0))
            .child(text("Your designs").text_size(px(12.0)).text_color(theme::MUTED))
            .child(match removing {
                Some((i, name)) => confirm_row(
                    format!("Remove “{name}” from Your designs?"),
                    "Remove",
                    move || store().design_forget(i),
                    || store().design_removing.set(None),
                )
                .into_element(),
                None => view().into_element(),
            })
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .justify_center()
                    .gap_2()
                    .children(saved.into_iter().enumerate().map(|(i, c)| {
                        let on = current.as_ref().is_some_and(|sh| *sh == c.shape);
                        let hov = signal(false);
                        let use_it = c.clone();
                        view()
                            .size(px(48.0))
                            .items_center()
                            .justify_center()
                            .label(format!("Use your {} design", c.name))
                            .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
                            .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
                            .on_press(move || store().design_use(use_it.clone(), for_new, false))
                            .child(when(
                                move || on,
                                || {
                                    view()
                                        .absolute()
                                        .size(px(46.0))
                                        .rounded(px(23.0))
                                        .border_1()
                                        .border_color(theme::BORDER_S)
                                        .into_element()
                                },
                            ))
                            .child(bot_blob(BotBlobProps {
                                shape: c.shape,
                                color: ink.clone(),
                                size: 44.0,
                                paused: true,
                                ..Default::default()
                            }))
                            .child(when(
                                move || hov.get(),
                                move || {
                                    view()
                                        .absolute()
                                        .top(px(-4.0))
                                        .right(px(-4.0))
                                        .size(px(18.0))
                                        .rounded(px(9.0))
                                        .bg(theme::MENU_BG)
                                        .border_1()
                                        .border_color(theme::BORDER_S)
                                        .items_center()
                                        .justify_center()
                                        .child(text("✕").text_size(px(10.0)).text_color(theme::DIM))
                                        .label("Remove this design")
                                        // Asks first: the confirm row below removes.
                                        .on_press(move || store().design_removing.set(Some(i)))
                                        .into_element()
                                },
                            ))
                            .into_element()
                    })),
            )
            .into_element()
    })
}

/// An inline "are you sure?" row: the question, Keep (cancel) and the
/// destructive action in red. Nothing happens until one is pressed.
pub(crate) fn confirm_row(
    question: String,
    action: &'static str,
    on_do: impl Fn() + 'static,
    on_keep: impl Fn() + 'static,
) -> impl IntoElement {
    view()
        .w_full()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_2()
        .py(px(8.0))
        .px(px(10.0))
        .rounded(px(10.0))
        .bg(theme::SUBTLE)
        .border_1()
        .border_color(theme::BORDER_S)
        .child(text(question).flex_1().text_size(px(12.0)).text_color(theme::TEXT))
        .child(
            view()
                .h(px(24.0))
                .px(px(10.0))
                .rounded(px(12.0))
                .bg(theme::HOVER)
                .justify_center()
                .child(text("Keep").text_size(px(12.0)).text_color(theme::TEXT))
                .label("Keep it")
                .on_press(on_keep),
        )
        .child(
            view()
                .h(px(24.0))
                .px(px(10.0))
                .rounded(px(12.0))
                .bg(theme::DOWN_TINT)
                .justify_center()
                .child(text(action).text_size(px(12.0)).text_color(theme::DOWN))
                .label(action)
                .on_press(on_do),
        )
}
