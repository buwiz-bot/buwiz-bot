//! The hot area: the whole chat shell, rebuilt on every patch. Keep
//! patchable state out of statics and thread-locals — the model lives in
//! `live::store()`'s anchor and small ephemeral signals go through
//! `live::keep`. A patch that changes a type the program can see is
//! answered `Restart` by the daemon, not applied.

use nsh_core::ui::prelude::*;

use crate::chat;
use crate::composer;
use crate::ctx;
use crate::files;
use crate::icons::icon;
use crate::live::store;
use crate::models;
use crate::palette;
use crate::pane;
use crate::sidebar;
use crate::theme;

/// The app: sidebar · transcript · composer · detail pane, plus the
/// overlay layers (palette, model picker, ctx menu, file viewer).
pub(crate) fn hot_area() -> impl IntoElement {
    let st = store();
    st.boot();

    // ⌘K palette, ⌘N new chat — the shell's bindings.
    on_key("mod+k", |_| store().palette.set(true));
    on_key("mod+n", |_| store().new_thread());
    on_key("mod+b", |_| {
        let st = store();
        st.side_mode
            .set(if st.side_mode.get_untracked() == "full" { "rail" } else { "full" }.to_owned());
    });

    // Appearance follows the store's `dark` toggle.
    effect(|| {
        set_color_scheme(if store().dark.get() {
            Appearance::Dark
        } else {
            Appearance::Light
        });
    });

    // Hidden title bar on the desktop, keeping the traffic lights over
    // the sidebar's drag strip.
    #[cfg(target_os = "macos")]
    nsh_core::ui::desktop::set_title_bar(
        nsh_core::desktop::TitleBarStyle::Hidden,
        Some(px(44.0)),
    );

    view()
        .flex_1()
        .h_full()
        .flex_row()
        .bg(theme::MAIN)
        .child(sidebar::sidebar())
        .child(
            // Main column: transcript + floating top bar + composer dock.
            view()
                .flex_1()
                .h_full()
                .child(dynamic(|| {
                    if store().turns.get().is_empty() {
                        chat::empty_state().into_element()
                    } else {
                        chat::transcript().into_element()
                    }
                }))
                .child(top_bar())
                .child(composer::composer()),
        )
        .child(
            when(move || store().right_open.get(), || {
                pane::pane().into_element()
            }),
        )
        .child(ctx::layer())
        .child(palette::layer())
        .child(models::layer())
        .child(files::layer())
}

/// The floating top bar: thread title + actions, over the transcript.
fn top_bar() -> impl IntoElement {
    let hovered = signal(false);
    view()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .right(px(0.0))
        .h(px(52.0))
        .flex_row()
        .items_center()
        .px_4()
        .pt(px(4.0))
        .window_drag(true)
        .child(
            text(move || {
                let st = store();
                let id = st.active_id.get();
                st.threads
                    .get()
                    .iter()
                    .find(|t| t.id == id)
                    .map(|t| if t.name.is_empty() { "New chat".to_owned() } else { t.name.clone() })
                    .unwrap_or_default()
            })
            .text_size(px(13.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::TEXT)
            .number_of_lines(1),
        )
        .child(view().flex_1())
        .child(
            view()
                .label("Details")
                .child(
                    view()
                        .size(px(30.0))
                        .rounded(px(8.0))
                        .items_center()
                        .justify_center()
                        .bg(move || {
                            if hovered.get() {
                                theme::HOVER
                            } else {
                                AdaptiveColor::TRANSPARENT
                            }
                        })
                        .child(icon("panel")),
                )
                .on_hover_in(move || hovered.set(true))
                .on_hover_out(move || hovered.set(false))
                .on_press(|| store().right_open.update(|v| *v = !*v)),
        )
}
