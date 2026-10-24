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
use crate::plugins;
use crate::pane;
use crate::sidebar;
use crate::theme;

/// The app: sidebar · transcript · composer · detail pane, plus the
/// overlay layers (palette, model picker, ctx menu, file viewer).
pub(crate) fn hot_area() -> impl IntoElement {
    let st = store();
    st.boot();
    st.watch_plugins();

    // `ROX_STATUS=<state|demo>`: debug-mount the run-status surface —
    // the composer status row and bot marks play the state without a
    // `/status:` send (same `agentSet` path, no server).
    if let Ok(s) = std::env::var("ROX_STATUS") {
        spawn(async move {
            timer(std::time::Duration::from_millis(1500)).await;
            store().mock_status(&s);
        });
    }

    // `ROX_PANE=1`: open the right detail pane at boot — keypress
    // automation can't reach the window, so screenshots need the env.
    if std::env::var("ROX_PANE").is_ok() {
        spawn(async move {
            timer(std::time::Duration::from_millis(800)).await;
            store().right_open.set(true);
        });
    }

    // `ROX_WORK=1`: open the Mini Apps work tab at boot.
    if std::env::var("ROX_WORK").is_ok() {
        spawn(async move {
            timer(std::time::Duration::from_millis(800)).await;
            store().tab.set("work".to_owned());
        });
    }

    // `ROX_PLUGINS=1`: open the Connect Apps modal at boot.
    if std::env::var("ROX_PLUGINS").is_ok() {
        spawn(async move {
            timer(std::time::Duration::from_millis(900)).await;
            store().open_plugins();
        });
    }

    // `ROX_MODELS=effort|list`: open the model popover at boot.
    if let Ok(mode) = std::env::var("ROX_MODELS") {
        spawn(async move {
            timer(std::time::Duration::from_millis(900)).await;
            crate::models::dev_open(&mode);
        });
    }

    // `ROX_SIDE=rail|full`: pin the sidebar mode for screenshots.
    if let Ok(m) = std::env::var("ROX_SIDE") {
        store().side_shown.set(true);
        store().side_mode.set(m);
    }

    // `ROX_ACCOUNT=1`: open the account popover at boot.
    if std::env::var("ROX_ACCOUNT").is_ok() {
        spawn(async move {
            timer(std::time::Duration::from_millis(900)).await;
            store().account_open.set(true);
        });
    }

    // `ROX_TAB=library|details|…`: pin the detail pane's tab.
    if let Ok(tb) = std::env::var("ROX_TAB") {
        store().detail_tab.set(tb);
    }

    // `ROX_AVATAR=1`: open the avatar editor card at boot (after any
    // `ROX_OPEN` thread lands so `activeBot` exists).
    if std::env::var("ROX_AVATAR").is_ok() {
        spawn(async move {
            timer(std::time::Duration::from_millis(2600)).await;
            store().open_avatar_editor();
        });
    }

    // `ROX_OPEN=<thread>[,<thread>…]`: open thread(s) at boot for
    // screenshots — `open_thread` is the row-press path (select + turns +
    // apps). A list replays an agent switch with a beat between opens.
    if let Ok(t) = std::env::var("ROX_OPEN") {
        if !t.is_empty() {
            spawn(async move {
                timer(std::time::Duration::from_millis(1200)).await;
                for (i, id) in t.split(',').enumerate() {
                    if i > 0 {
                        timer(std::time::Duration::from_millis(900)).await;
                    }
                    store().open_thread(id.trim().to_owned());
                }
            });
        }
    }

    // `ROX_SEND=<text>`: fill the draft and send at boot — synthetic
    // input can't drive the composer, so stream verification needs it.
    if let Ok(msg) = std::env::var("ROX_SEND") {
        if !msg.is_empty() {
            spawn(async move {
                timer(std::time::Duration::from_millis(1200)).await;
                store().draft.set(msg);
                store().send();
            });
        }
    }

    // `ROX_PALETTE=<query>`: open the ⌘K card at boot — synthetic keys
    // can't reach `on_key`, so palette verification needs it.
    if let Ok(q) = std::env::var("ROX_PALETTE") {
        spawn(async move {
            timer(std::time::Duration::from_millis(1400)).await;
            let st = store();
            st.toggle_palette();
            if !q.is_empty() {
                st.palette_q.set(q);
            }
        });
    }

    // `ROX_SLOTS=1`: print the ⌘1…9 slot order once the model loads.
    if std::env::var("ROX_SLOTS").is_ok() {
        spawn(async move {
            timer(std::time::Duration::from_millis(1500)).await;
            let st = store();
            let slots = st.shortcut_threads();
            let threads = st.threads.get_untracked();
            let bots = st.bots.get_untracked();
            for (i, id) in slots.iter().enumerate() {
                let name = threads
                    .iter()
                    .find(|t| t.id == *id)
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                let bot = bots
                    .iter()
                    .find(|b| b.thread_id == *id)
                    .map(|b| format!("{} (pinned={} section={} hidden={})", b.name, b.pinned, b.section, b.hidden))
                    .unwrap_or_else(|| "chat".to_owned());
                eprintln!("[slot {}] {} — {} — {}", i + 1, id, name, bot);
            }
        });
    }

    // The shell's keymap (App.tsx:795): palette, sidebar show / mode,
    // details, theme, the New-Bot sheet, the work tab, dictation demo,
    // ⌘R reload (the web app's reload) and mod+1…9 chat switching.
    on_key("mod+k", |_| store().toggle_palette());
    on_key("mod+r", |_| store().reset_fresh());
    on_key("mod+b", |_| store().side_shown.update(|v| *v = !*v));
    on_key("mod+shift+h", |_| {
        let st = store();
        st.side_shown.set(true);
        st.side_mode
            .set(if st.side_mode.get_untracked() == "full" { "rail" } else { "full" }.to_owned());
    });
    on_key("mod+shift+b", |_| store().right_open.update(|v| *v = !*v));
    on_key("mod+t", |_| {
        let st = store();
        st.set_theme(st.next_theme());
    });
    on_key("mod+n", |_| store().bot_modal.set(true));
    on_key("mod+shift+m", |_| {
        let st = store();
        st.tab
            .set(if st.tab.get_untracked() == "work" { "chat" } else { "work" }.to_owned());
    });
    on_key("mod+d", |_| store().dictate_stub());
    on_key("escape", |_| {
        let st = store();
        if st.palette.get_untracked() {
            st.close_palette();
        }
        st.shelf_renaming.set(String::new());
        st.shelf_tagging.set(String::new());
        st.avatar_editing.set(false);
        // lightbox + fullscreen viewer (lightbox.tsx:98, fileviewer.tsx:783)
        st.light_close();
        st.viewing.set(None);
        crate::models::close();
        crate::composer::plus_menu().set(None);
        // the plugins modal pops a page, or closes at the market page.
        if st.plugins_open.get_untracked() {
            if st.plugin_nav.get_untracked().len() > 1 {
                st.pop_nav();
            } else {
                st.close_plugins();
            }
        }
    });
    // lightbox prev/next + table navigation / TSV copy — the viewer's
    // keydown listener (fileviewer.tsx:783).
    on_key("left", |_| {
        let st = store();
        if !st.light_items.get_untracked().is_empty() {
            st.light_step(-1);
        } else if st.viewing.get_untracked().map(|v| v.kind == "table").unwrap_or(false) {
            st.sel_move(0, -1, false);
        }
    });
    on_key("right", |_| {
        let st = store();
        if !st.light_items.get_untracked().is_empty() {
            st.light_step(1);
        } else if st.viewing.get_untracked().map(|v| v.kind == "table").unwrap_or(false) {
            st.sel_move(0, 1, false);
        }
    });
    on_key("up", |_| {
        let st = store();
        // palette.tsx:33 — the palette's arrow nav first.
        if st.palette.get_untracked() {
            let n = st.palette_results().len();
            if n > 0 {
                st.palette_sel.update(|i| *i = (*i + n as i64 - 1) % n as i64);
            }
        // the search combo's arrow nav (App.tsx:1416) rides the same
        // keys while the drop is open.
        } else if st.plugins_open.get_untracked() && st.drop_open() {
            let n = st.plugin_list().len();
            if n > 0 {
                st.drop_sel.update(|i| *i = (*i + n - 1) % n);
            }
        } else if st.viewing.get_untracked().map(|v| v.kind == "table").unwrap_or(false) {
            st.sel_move(-1, 0, false);
        }
    });
    on_key("down", |_| {
        let st = store();
        if st.palette.get_untracked() {
            let n = st.palette_results().len();
            if n > 0 {
                st.palette_sel.update(|i| *i = (*i + 1) % n as i64);
            }
        } else if st.plugins_open.get_untracked() && st.drop_open() {
            let n = st.plugin_list().len();
            if n > 0 {
                st.drop_sel.update(|i| *i = (*i + 1) % n);
            }
        } else if st.viewing.get_untracked().map(|v| v.kind == "table").unwrap_or(false) {
            st.sel_move(1, 0, false);
        }
    });
    on_key("enter", |_| {
        let st = store();
        if st.palette.get_untracked() {
            st.palette_run_at(st.palette_sel.get_untracked() as usize);
        } else if st.plugins_open.get_untracked() && st.drop_open() {
            st.drop_choose();
        }
    });
    on_key("shift+left", |_| {
        let st = store();
        if st.viewing.get_untracked().map(|v| v.kind == "table").unwrap_or(false) {
            st.sel_move(0, -1, true);
        }
    });
    on_key("shift+right", |_| {
        let st = store();
        if st.viewing.get_untracked().map(|v| v.kind == "table").unwrap_or(false) {
            st.sel_move(0, 1, true);
        }
    });
    on_key("shift+up", |_| {
        let st = store();
        if st.viewing.get_untracked().map(|v| v.kind == "table").unwrap_or(false) {
            st.sel_move(-1, 0, true);
        }
    });
    on_key("shift+down", |_| {
        let st = store();
        if st.viewing.get_untracked().map(|v| v.kind == "table").unwrap_or(false) {
            st.sel_move(1, 0, true);
        }
    });
    on_key("mod+c", |_| {
        let st = store();
        if st.sel_b.get_untracked().is_some() {
            st.copy_sel();
        }
    });
    // `mod+1…9` (App.tsx:797) — the visible sidebar order: pinned,
    // groups A→Z by latest interaction, Unassigned, then chats. While
    // the palette has a query they run that result instead
    // (palette.tsx:48).
    for k in 1..=9u8 {
        on_key(&format!("mod+{k}"), move |_| {
            let st = store();
            if st.palette.get_untracked() {
                if !st.palette_q.get_untracked().trim().is_empty() {
                    st.palette_run_at((k - 1) as usize);
                }
                return;
            }
            if let Some(id) = st.shortcut_threads().get((k - 1) as usize) {
                st.open_thread(id.clone());
            }
        });
    }

    // Appearance follows `themePref` (theme.ts): System resolves to the
    // OS scheme inside the engine.
    effect(|| {
        set_color_scheme(match store().theme_pref.get().as_str() {
            "light" => Appearance::Light,
            "dark" => Appearance::Dark,
            _ => Appearance::System,
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
    // `ROX_GAZE=1`: two resting blobs pinned to opposite gaze targets —
    // verifies set_gaze → unsettled → advance → paint_rest end to end.
    if std::env::var("ROX_GAZE").is_ok() {
        return view()
            .flex_1()
            .h_full()
            .bg(theme::MAIN)
            .items_center()
            .justify_center()
            .flex_row()
            .gap(px(40.0))
            .child(crate::avatar::bot_blob(crate::avatar::BotBlobProps {
                shape: "blob".to_owned(),
                color: "#FFFFFF".to_owned(),
                size: 80.0,
                state: "idle",
                gaze_at: Some([-0.9, 0.9]),
                ..Default::default()
            }))
            .child(crate::avatar::bot_blob(crate::avatar::BotBlobProps {
                shape: "blob".to_owned(),
                color: "#FFFFFF".to_owned(),
                size: 80.0,
                state: "idle",
                gaze_at: Some([0.9, -0.9]),
                ..Default::default()
            }));
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
                    let st = store();
                    if st.tab.get() == "work" {
                        crate::shelf::work_tab().into_element()
                    } else if st.turns.get().is_empty() {
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
        .child(sidebar::account_pop_layer())
        .child(pane::rename_overlay())
        .child(pane::editor_overlay())
        .child(botmodal::layer())
        .child(palette::layer())
        .child(models::layer())
        .child(composer::plus_layer())
        .child(files::layer())
        .child(files::lightbox_layer())
        .child(plugins::layer())
}

/// The floating top bar (App.tsx:2375): the centered header pill —
/// `BotAvatar(24/22 paused)` + the bot's name + a hover chevron — pressing
/// it toggles the detail pane. Grok has no right-side control here.
fn top_bar() -> impl IntoElement {
    let pill_hover = signal(false);
    view()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .right(px(0.0))
        .h(px(52.0))
        .window_drag(true)
        .child(
            // Center layer — the pill is `position:absolute` centered in
            // the TSX so side controls never shift it.
            view()
                .absolute()
                .size_full()
                .items_center()
                .justify_center()
                .window_drag(true)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .h(px(38.0))
                        .pl(move || {
                            if store().active_bot().is_some() {
                                px(8.0)
                            } else {
                                px(16.0)
                            }
                        })
                        .pr(move || {
                            if pill_hover.get() && !store().right_open.get() {
                                px(10.0)
                            } else {
                                px(16.0)
                            }
                        })
                        .rounded(px(19.0))
                        .bg(move || {
                            if pill_hover.get() {
                                theme::HOVER
                            } else {
                                theme::CARD2
                            }
                        })
                        .border(px(0.5))
                        .border_color(theme::BORDER_F)
                        .on_hover_in(move || pill_hover.set(true))
                        .on_hover_out(move || pill_hover.set(false))
                        .on_press(|| store().right_open.update(|v| *v = !*v))
                        .label(move || {
                            store()
                                .active_bot()
                                .map(|b| b.name)
                                .unwrap_or_else(|| "Rox".to_owned())
                        })
                        .child(dynamic(move || {
                            let Some(b) = store().active_bot() else {
                                return view().into_element();
                            };
                            crate::avatar::bot_avatar(crate::avatar::BotAvatarProps {
                                size: 24.0,
                                box_: Some(22.0),
                                paused: true,
                                ..crate::avatar::BotAvatarProps::bot(&b, 24.0)
                            })
                            .into_element()
                        }))
                        .child(
                            text(move || {
                                store()
                                    .active_bot()
                                    .map(|b| b.name)
                                    .unwrap_or_else(|| "Rox".to_owned())
                            })
                            .text_size(px(14.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::TEXT)
                            .number_of_lines(1),
                        )
                        .child(when(
                            move || pill_hover.get() && !store().right_open.get(),
                            || icon("chev-right").into_element(),
                        )),
                ),
        )
}
