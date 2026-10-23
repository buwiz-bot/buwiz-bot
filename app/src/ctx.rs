//! The context-menu card (`ctxMenu` in the TSX shell): a small popover at
//! the pointer with a list of actions. The store carries `ctx`; a tap on
//! the scrim or an item closes it.

use nsh_core::ui::prelude::*;

use crate::{live::store, store::CtxMenu, theme};
use crate::icons::icon;

/// Run a ctx item's `act` string — the dispatch table the TSX built
/// inline. Items carry a verb; verbs the shell knows are wired here.
fn act(verb: &str) {
    let st = store();
    st.ctx.set(None);
    match verb {
        "new" => st.new_thread(),
        "delete" => {
            let id = st.active_id.get_untracked();
            if !id.is_empty() {
                st.delete_thread(id);
            }
        }
        "palette" => st.palette.set(true),
        "models" => st.models_open.set(true),
        _ => {}
    }
}

/// The overlay layer that shows the open `ctx` menu, if any.
pub(crate) fn layer() -> impl IntoElement {
    dynamic(|| {
        let Some(menu) = store().ctx.get() else {
            return view().into_element();
        };
        ctx_card(menu).into_element()
    })
}

fn ctx_card(menu: CtxMenu) -> impl IntoElement {
    let items = menu.items.clone();
    view()
        .size_full()
        .absolute()
        .child(
            // Scrim: dismiss on press anywhere.
            view()
                .size_full()
                .on_press(|| store().ctx.set(None)),
        )
        .child(
            view()
                .absolute()
                .left(px(menu.x))
                .top(px(menu.y))
                .w(px(180.0))
                .bg(theme::MENU_BG)
                .rounded(px(10.0))
                .border_1()
                .border_color(theme::BORDER_S)
                .py_1()
                .child(each(
                    move || items.clone(),
                    |it| it.label.clone(),
                    |it, _| ctx_row(it.get()),
                )),
        )
}

fn ctx_row(it: crate::store::CtxItem) -> impl IntoElement {
    let verb = it.act.clone();
    let color = if it.danger { theme::DOWN } else { theme::TEXT };
    let hovered = signal(false);
    view()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py(px(6.0))
                .bg(move || if hovered.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
                .child(icon(&it.icon))
                .child(text(it.label).text_size(px(13.0)).text_color(color)),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || act(&verb))
}
