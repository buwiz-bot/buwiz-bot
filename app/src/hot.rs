//! The hot area: the whole chat shell, rebuilt on every patch. Keep
//! patchable state out of statics and thread-locals — the model lives in
//! `live::store()`'s anchor and small ephemeral signals go through
//! `live::keep`. A patch that changes a type the program can see is
//! answered `Restart` by the daemon, not applied.

use nsh_core::ui::prelude::*;

use crate::botmodal;
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

    // `ROX_STATUS=<state|demo>`: debug-mount the run-status surface —
    // the composer status row and bot marks play the state without a
    // `/status:` send (same `agentSet` path, no server).
    if let Ok(s) = std::env::var("ROX_STATUS") {
        spawn(async move {
            timer(std::time::Duration::from_millis(1500)).await;
            store().mock_status(&s);
        });
    }

    // The shell's keymap (App.tsx:795): palette, sidebar show / mode,
    // details, theme, the New-Bot sheet, the work tab, dictation demo
    // and mod+1…9 chat switching.
    on_key("mod+k", |_| store().palette.set(true));
    on_key("mod+b", |_| store().side_shown.update(|v| *v = !*v));
    on_key("mod+shift+h", |_| {
        let st = store();
        st.side_shown.set(true);
        st.side_mode
            .set(if st.side_mode.get_untracked() == "full" { "rail" } else { "full" }.to_owned());
    });
    on_key("mod+shift+b", |_| store().right_open.update(|v| *v = !*v));
    on_key("mod+t", |_| store().dark.update(|v| *v = !*v));
    on_key("mod+n", |_| store().bot_modal.set(true));
    on_key("mod+shift+m", |_| {
        let st = store();
        st.tab
            .set(if st.tab.get_untracked() == "work" { "chat" } else { "work" }.to_owned());
    });
    on_key("mod+d", |_| {
        let st = store();
        let tid = st.active_id.get_untracked();
        if tid.is_empty() {
            return;
        }
        let bot = st.active_bot_untracked().map(|b| b.id);
        st.agent_set(
            tid.clone(),
            bot,
            Some(("dictating".to_owned(), "Dictation is coming soon".to_owned())),
        );
        spawn(async move {
            timer(std::time::Duration::from_millis(2400)).await;
            let st = store();
            if st.agent().map(|a| a.label) == Some("Dictation is coming soon".to_owned()) {
                st.agent_set(tid, st.active_bot_untracked().map(|b| b.id), None);
            }
        });
    });
    for k in 1..=9u8 {
        on_key(&format!("mod+{k}"), move |_| {
            let st = store();
            if let Some(t) = st.chats().get((k - 1) as usize) {
                st.open_thread(t.id.clone());
            }
        });
    }

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

    // `ROX_MINI=R<n>`: debug-mount a generated widget alone in the window —
    // the same fetch → compile → AppView path the chat cards take, without
    // needing a widget turn to be on screen. `ROX_CARD=R<n>` mounts the
    // turn's whole card instead, chrome and fixed body height included.
    if let Ok(app) = std::env::var("ROX_MINI") {
        if !app.is_empty() {
            return view()
                .flex_1()
                .h_full()
                .bg(theme::MAIN)
                .child(crate::mini::mini_app(&app, 0.0));
        }
    }
    if let Ok(app) = std::env::var("ROX_CARD") {
        if !app.is_empty() {
            return view()
                .flex_1()
                .h_full()
                .bg(theme::MAIN)
                .p_4()
                .child(crate::widgets::app_card(crate::store::Art {
                    app,
                    title: "debug card".to_owned(),
                    desc: String::new(),
                    ..Default::default()
                }));
        }
    }

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
        .child(ctx::prompt_layer())
        .child(sidebar::rail_pop_layer())
        .child(botmodal::layer())
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
            // `BotAvatar` in Grok's chat-header pill: static (paused), the
            // thread's bot mark when the thread has a persona.
            when(move || store().active_bot().is_some(), || {
                let b = store().active_bot_untracked().unwrap_or_default();
                crate::avatar::bot_avatar(crate::avatar::BotAvatarProps {
                    size: 24.0,
                    box_: Some(22.0),
                    paused: true,
                    ..crate::avatar::BotAvatarProps::bot(&b, 24.0)
                })
            }),
        )
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
