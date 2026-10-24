//! The ⌘K palette (`palette.tsx`): a centered card listing the shell's
//! commands; Escape or the scrim dismisses.

use nsh_core::ui::prelude::*;

use crate::icons::icon;
use crate::live::store;
use crate::theme;

#[derive(Clone, PartialEq)]
struct Command {
    label: &'static str,
    icon: &'static str,
    hint: &'static str,
    act: &'static str,
}

fn commands() -> Vec<Command> {
    vec![
        Command { label: "New chat", icon: "chat", hint: "", act: "new" },
        Command { label: "New bot", icon: "agent", hint: "", act: "bot" },
        Command { label: "Choose model", icon: "agent", hint: "", act: "models" },
        Command { label: "Toggle sidebar", icon: "panel", hint: "", act: "side" },
        Command { label: "Toggle details", icon: "panel", hint: "", act: "details" },
        Command { label: "Toggle theme", icon: "settings", hint: "", act: "theme" },
    ]
}

fn run(act: &str) {
    let st = store();
    st.palette.set(false);
    match act {
        "new" => st.new_thread(),
        "bot" => st.bot_modal.set(true),
        "models" => crate::models::open_list(),
        "side" => st
            .side_mode
            .set(if st.side_mode.get_untracked() == "full" { "rail" } else { "full" }.to_owned()),
        "details" => st.right_open.update(|v| *v = !*v),
        "theme" => {
            let next = st.next_theme();
            st.set_theme(next);
        }
        _ => {}
    }
}

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
    let card = view()
        .w(px(420.0))
        .bg(theme::MENU_BG)
        .rounded(px(12.0))
        .border_1()
        .border_color(theme::BORDER_S)
        .py_1()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .border_b_1()
                .border_b_color(theme::BORDER_F)
                .child(icon("search"))
                .child(
                    text("Commands")
                        .text_size(px(13.0))
                        .text_color(theme::MUTED),
                ),
        )
        .child(each(commands, |c| c.label, |c, _| palette_row(c.get())));
    view()
        .size_full()
        .absolute()
        .child(view().size_full().on_press(|| store().palette.set(false)))
        .child(
            view()
                .size_full()
                .pointer_events(PointerEvents::BoxNone)
                .items_start()
                .justify_center()
                .pt(px(120.0))
                .child(card),
        )
}

fn palette_row(c: Command) -> impl IntoElement {
    let hovered = signal(false);
    let act = c.act;
    view()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py(px(8.0))
                .bg(move || if hovered.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
                .child(crate::icons::icon_sized(c.icon, 13.0))
                .child(
                    text(c.label)
                        .text_size(px(13.0))
                        .text_color(theme::TEXT)
                        .flex_1(),
                ),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || run(act))
}
