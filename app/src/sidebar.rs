//! The left column (`shell.tsx`'s sidebar): the app title, "New chat",
//! the thread list, and the agents catalog at the bottom.

use nsh_core::ui::prelude::*;

use crate::avatar::bot_avatar;
use crate::icons::icon;
use crate::live::store;
use crate::store::{BotMeta, ThreadMeta};
use crate::theme;

/// The rail/full sidebar — `side_mode` picks slim icons or the list.
pub(crate) fn sidebar() -> impl IntoElement {
    let mode = move || store().side_mode.get();
    when(move || mode() == "full", full_sidebar)
        .otherwise(rail)
}

fn rail() -> impl IntoElement {
    view()
        .w(px(52.0))
        .h_full()
        .bg(theme::SIDEBAR)
        .border_r_1()
        .border_r_color(theme::BORDER_F)
        .items_center()
        .pt(px(46.0)) // clear the hidden title bar's traffic lights
        .gap_2()
        .child(rail_button("chat", "New chat", || store().new_thread()))
        .child(rail_button("search", "Palette", || store().palette.set(true)))
        .child(
            view().flex_1(),
        )
        .child(rail_button("settings", "Models", || store().models_open.set(true)))
        .pb_2()
}

fn rail_button(name: &'static str, label: &'static str, on: impl Fn() + 'static) -> impl IntoElement {
    let hovered = signal(false);
    view()
        .label(label)
        .child(
            view()
                .size(px(36.0))
                .rounded(px(9.0))
                .items_center()
                .justify_center()
                .bg(move || if hovered.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
                .child(icon(name)),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || on())
}

fn full_sidebar() -> impl IntoElement {
    view()
        .w(px(250.0))
        .h_full()
        .bg(theme::SIDEBAR)
        .border_r_1()
        .border_r_color(theme::BORDER_F)
        .child(
            // Header — doubles as the window drag strip.
            view()
                .flex_row()
                .items_center()
                .px_4()
                .pt(px(46.0))
                .pb_2()
                .window_drag(true)
                .child(
                    text("Rox")
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::TEXT)
                        .flex_1(),
                )
                .child(head_button("plus", "New chat", || store().new_thread()))
                .child(head_button("panel", "Collapse", || {
                    store().side_mode.set("rail".to_owned())
                })),
        )
        .child(
            scroll().flex_1().child(
                view()
                    .px_2()
                    .pb_2()
                    .child(section_label("Chats"))
                    .child(each(
                        || store().threads.get(),
                        |t| t.id.clone(),
                        |t, _| thread_row(t.get()),
                    ))
                    .child(section_label("Agents"))
                    .child(each(
                        || store().bots.get(),
                        |b| b.id.clone(),
                        |b, _| bot_row(b.get()),
                    )),
            ),
        )
}

fn section_label(label: &'static str) -> impl IntoElement {
    text(label)
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::GHOST)
        .px_2()
        .pt(px(14.0))
        .pb(px(4.0))
}

fn head_button(name: &'static str, label: &'static str, on: impl Fn() + 'static) -> impl IntoElement {
    let hovered = signal(false);
    view()
        .label(label)
        .child(
            view()
                .size(px(26.0))
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

fn thread_row(t: ThreadMeta) -> impl IntoElement {
    let st = store();
    let id = t.id.clone();
    let tid = t.id.clone();
    let hovered = signal(false);
    let active = move || st.active_id.get() == tid;
    view()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py(px(7.0))
                .rounded(px(8.0))
                .bg(move || {
                    if active() {
                        theme::ACTIVE
                    } else if hovered.get() {
                        theme::HOVER
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .child(
                    text(if t.name.is_empty() { "New chat".to_owned() } else { t.name.clone() })
                        .text_size(px(13.0))
                        .text_color(theme::TEXT)
                        .flex_1()
                        .number_of_lines(1),
                ),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || st.open_thread(id.clone()))
}

fn bot_row(b: BotMeta) -> impl IntoElement {
    let hovered = signal(false);
    let app = b.id.clone();
    let name = b.name.clone();
    let desc = b.desc.clone();
    view()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py(px(7.0))
                .rounded(px(8.0))
                .bg(move || if hovered.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
                .child(bot_avatar(b, 22.0))
                .child(
                    view()
                        .flex_1()
                        .child(
                            text(name)
                                .text_size(px(13.0))
                                .text_color(theme::TEXT)
                                .number_of_lines(1),
                        )
                        .child(
                            text(desc)
                                .text_size(px(11.0))
                                .text_color(theme::GHOST)
                                .number_of_lines(1),
                        ),
                ),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || {
            let st = store();
            st.load_detail(app.clone());
            st.right_open.set(true);
        })
}
