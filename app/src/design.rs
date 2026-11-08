//! The person's custom avatars ("Your designs", `Store::design_saved`): the
//! Custom tab of the Create New Bot sheet and of Edit Avatar. `for_new` says
//! where a tap lands (the sheet's draft, or the bot). With none there, the
//! tab says how to get some: `/avatars install all` in chat.

use nsh_core::prelude::*;

use crate::avatar::{bot_blob, BotBlobProps};
use crate::live::store;
use crate::theme;

/// What the empty Custom tab offers to put in chat.
pub(crate) const INSTALL_ALL: &str = "/avatars install all";

/// The Custom tab's body: a loading line until the list is read, the empty
/// state when it holds nothing, else the avatars ([`saved`]).
pub(crate) fn custom(for_new: bool) -> impl IntoElement {
    dynamic(move || {
        let st = store();
        if !st.design_saved.with(Vec::is_empty) {
            return saved(for_new).into_element();
        }
        if !st.design_loaded.get() {
            return text("Loading your avatars…")
                .w_full()
                .text_center()
                .text_size(px(13.0))
                .text_color(theme::MUTED)
                .into_element();
        }
        empty(for_new).into_element()
    })
}

/// No custom avatars yet: a muted face, what to do, and the command with
/// Insert in chat (closes the sheet or Edit Avatar and puts it in the
/// message box, unsent) and Copy.
fn empty(for_new: bool) -> impl IntoElement {
    let copied = signal(false);
    view()
        .w_full()
        .items_center()
        .justify_center()
        .gap_2()
        .py_3()
        .child(crate::icons::icon_ink("smile", 32.0, || theme::GHOST))
        .child(
            text("No custom avatars yet")
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::TEXT),
        )
        .child(
            text("Install a pack in chat. They show up here, and in Edit Avatar.")
                .text_size(px(12.0))
                .text_color(theme::MUTED)
                .text_center(),
        )
        .child(
            view()
                .mt_1()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    text(INSTALL_ALL)
                        .text_size(px(12.0))
                        .text_color(theme::TEXT)
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(6.0))
                        .bg(theme::SUBTLE),
                )
                .child(small_pill(
                    || "Insert in chat".to_owned(),
                    "Insert in chat",
                    move || insert_in_chat(for_new),
                ))
                .child(small_pill(
                    move || if copied.get() { "Copied".to_owned() } else { "Copy".to_owned() },
                    "Copy",
                    move || {
                        store().copy_text(INSTALL_ALL);
                        copied.set(true);
                    },
                )),
        )
}

fn small_pill(
    label: impl Fn() -> String + 'static,
    a11y: &'static str,
    on: impl Fn() + 'static,
) -> impl IntoElement {
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

/// Closes the sheet (or Edit Avatar) and puts [`INSTALL_ALL`] in the open
/// chat's message box, the caret at its end. Nothing is sent.
fn insert_in_chat(for_new: bool) {
    let st = store();
    if for_new {
        st.bot_modal.set(false);
    } else {
        st.close_avatar_editor();
    }
    st.draft.set(INSTALL_ALL.to_owned());
    crate::composer::focus();
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

/// The custom avatars, newest first, at rest and in the bot's colour (like
/// the kit grid). A tap puts the shape on the bot (or the sheet's draft),
/// which keeps its colour; hover shows ✕ to remove it. Nothing is drawn
/// while the list is empty.
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
