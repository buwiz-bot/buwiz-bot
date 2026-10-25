//! The hot area: the whole chat shell, rebuilt on every patch. Keep
//! patchable state out of statics and thread-locals — the model lives in
//! `live::store()`'s anchor and small ephemeral signals go through
//! `live::keep`. A patch that changes a type the program can see is
//! answered `Restart` by the daemon, not applied.

use nsh_core::ui::prelude::*;
use nsh_core::{TouchEvent, TouchPhase};

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
/// overlay layers (palette, model picker, ctx menu, file viewer). The
/// `remount`/`note` signals come from the root ([`App`]) so the live
/// pill keeps counting across remounts.
pub(crate) fn hot_area(remount: Signal<u64>, note: Signal<String>) -> impl IntoElement {
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

    // `ROX_SCROLL=<px>`: scroll the transcript to a pixel offset at
    // boot — `open_thread`'s scrollToEnd lands on the latest, so shots
    // of earlier content (the file cards) need a jump back.
    if let Ok(off) = std::env::var("ROX_SCROLL") {
        let off = off.trim().parse::<f32>().unwrap_or(0.0);
        spawn(async move {
            timer(std::time::Duration::from_millis(2400)).await;
            if let Some(l) = store().chat_list.borrow().clone() {
                l.scroll_to_offset(px(off), false);
            }
        });
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
    // ⌘⇧R toggles the ra-jit stats pill (a dev HUD, not in the TSX).
    let live_bar_show = crate::live::keep("rox.livebar.show", || true);
    on_key("mod+shift+r", move |_| live_bar_show.update(|v| *v = !*v));
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
                .child(crate::mini::mini_app(&app, 0.0, None, None));
        }
    }
    if let Ok(app) = std::env::var("ROX_CARD") {
        if !app.is_empty() {
            return view()
                .flex_1()
                .h_full()
                .bg(theme::MAIN)
                .p_4()
                .child(crate::widgets::app_card(crate::widgets::CardShell {
                    art: crate::store::Art {
                        app,
                        title: "debug card".to_owned(),
                        ..Default::default()
                    },
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

    // `ROX_PILL=1` (or `ROX_PILL=<turn index>`): open the emoji pill at a
    // fixed anchor — verifies the overlay renders, positions and paints
    // without a synthetic click. A turn index attaches picks to a real
    // turn so the tapback badge can be checked end to end.
    if let Ok(pill) = std::env::var("ROX_PILL") {
        let i = pill.parse::<i64>().unwrap_or(-1);
        spawn(async move {
            // Past the transcript's layout settle — its scroll ticks
            // legitimately close the pill.
            timer(std::time::Duration::from_millis(2000)).await;
            store().open_pill(crate::store::PillAnchor {
                x: 300.0,
                y: 300.0,
                w: 120.0,
                h: 24.0,
                flip: false,
                right: false,
                i,
                slot: "msg".to_owned(),
                current: "❤️".to_owned(),
            });
        });
    }

    // `ROX_OVERLAY=ctx|prompt|lightbox|plus|bot`: open an overlay at boot
    // — verifies each layer mounts and paints without synthetic input.
    if let Ok(ov) = std::env::var("ROX_OVERLAY") {
        spawn(async move {
            timer(std::time::Duration::from_millis(2000)).await;
            let st = store();
            match ov.as_str() {
                "ctx" => crate::ctx::open_at(
                    420.0,
                    320.0,
                    vec![
                        crate::store::CtxItem::row("Rename", "pencilLine", "copy", "x".into()),
                        crate::store::CtxItem::row("Duplicate", "copy", "copy", "y".into())
                            .on(true),
                        crate::store::CtxItem::separator(),
                        crate::store::CtxItem::danger("Delete", "trash", "copy", "z".into()),
                    ],
                ),
                "prompt" => st
                    .section_prompt
                    .set(Some(st.active_bot().map(|b| b.id).unwrap_or_default())),
                "plus" => crate::composer::plus_menu().set(Some((140.0, 620.0))),
                "bot" => st.bot_modal.set(true),
                "lightbox" => st.open_lightbox(
                    vec![crate::store::LightItem {
                        kind: "image".into(),
                        uri: "file:///tmp/rox_lightbox.png".into(),
                        name: "rox_lightbox.png".into(),
                        ..Default::default()
                    }],
                    0,
                ),
                _ => {}
            }
        });
    }

    // Only empty ↔ non-empty swaps the transcript for the empty state: a
    // dynamic reading `turns` itself rebuilt the whole list (and snapped it
    // to the end) on every reaction, poll and streamed chunk.
    let no_turns = computed(|| store().turns.with(|t| t.is_empty()));
    view()
        .flex_1()
        .h_full()
        .flex_row()
        .bg(theme::MAIN)
        // Click-away for the emoji pill, the native-menu way
        // (App.tsx:1927): touches propagate innermost-first, so the pill
        // marks `pillGuard` before this hears the same touch — a start
        // that arrives unmarked landed anywhere else and dismisses. No
        // overlay, so hovers and presses still reach every surface.
        .on_touch(|e: &TouchEvent| {
            if e.phase == TouchPhase::Start {
                let st = store();
                let inside = st.pill_guard.replace(false);
                if !inside {
                    st.close_pill(-1);
                }
            }
        })
        .child(sidebar::sidebar())
        .child(
            // Main column: transcript + floating top bar + composer dock.
            view()
                .flex_1()
                .h_full()
                .child(dynamic(move || {
                    let st = store();
                    if st.tab.get() == "work" {
                        crate::shelf::work_tab().into_element()
                    } else if no_turns.get() {
                        chat::empty_state().into_element()
                    } else {
                        chat::transcript().into_element()
                    }
                }))
                .child(top_bar())
                .child(composer::composer()),
        )
        .child(sidebar::hidden_toggle_layer())
        .child(
            when(move || store().right_open.get(), || {
                pane::pane().into_element()
            }),
        )
        // The emoji pill (zIndex 49) rides under the ctx menu (51).
        .child(chat::pill_layer())
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
        .child(stats_bar(remount, note, live_bar_show))
}

/// The ra-jit live pill: a status dot (green live · amber the edit needs
/// a restart · red a patch was declined · grey no ra-jit daemon) and ↻
/// with the remount count; hovering shows the last report. ⌘⇧R toggles
/// it and it drags anywhere; both survive remounts through `keep`.
fn stats_bar(remount: Signal<u64>, note: Signal<String>, show: Signal<bool>) -> impl IntoElement {
    let pos = crate::live::keep("rox.livebar.pos", || {
        (12.0f32, (window().height.0 - 40.0).max(0.0))
    });
    let dx = animated_value(0.0);
    let dy = animated_value(0.0);
    let hovered = signal(false);
    // `<state>:<message>` (live.rs `install`).
    let state = move || note.get().split_once(':').map(|(s, _)| s.to_owned()).unwrap_or_default();
    let message = move || note.get().split_once(':').map(|(_, m)| m.to_owned()).unwrap_or_default();
    dynamic(move || {
        if !show.get() {
            return view().into_element();
        }
        let (x, y) = pos.get();
        view()
            .absolute()
            .left(px(x))
            .top(px(y))
            .pan_values(Some(&dx), Some(&dy))
            .bind_translate_x(&dx)
            .bind_translate_y(&dy)
            .on_pan_end(move |e: &PanEvent| {
                let win = window();
                pos.update(|p| {
                    p.0 = (p.0 + e.translation.x.0).clamp(4.0, (win.width.0 - 60.0).max(4.0));
                    p.1 = (p.1 + e.translation.y.0).clamp(4.0, (win.height.0 - 26.0).max(4.0));
                });
                dx.set(0.0);
                dy.set(0.0);
            })
            .on_hover_in(move || hovered.set(true))
            .on_hover_out(move || hovered.set(false))
            .label(move || message())
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(10.0))
            .bg(theme::PILL_BG)
            .border(px(0.5))
            .border_color(theme::BORDER_F)
            .child(
                view().size(px(7.0)).rounded(px(3.5)).bg(move || match state().as_str() {
                    "live" | "applied" => theme::UP,
                    "restart" => theme::tone(
                        theme::hsla(0.09, 0.95, 0.45, 1.0),
                        theme::hsla(0.09, 0.95, 0.58, 1.0),
                    ),
                    "declined" => theme::DOWN,
                    _ => theme::GHOST,
                }),
            )
            .child(crate::icons::icon_ink("retry", 11.0, || theme::MUTED))
            .child(
                text(move || remount.get().to_string())
                    .text_size(px(11.0))
                    .font_family("monospace")
                    .text_color(theme::MUTED),
            )
            .child(when(
                move || hovered.get(),
                move || {
                    text(move || message())
                        .text_size(px(11.0))
                        .font_family("monospace")
                        .text_color(theme::TEXT)
                        .number_of_lines(1)
                        .max_w(px(420.0))
                        .into_element()
                },
            ))
            .into_element()
    })
}

/// The header pill's fills: `tc("#242424")` / `tc("#2c2c2c")` on dark,
/// `#ffffff` / `#f7f7f7` on light; the border `#ffffff14` / `#0000001a`.
const PILL: AdaptiveColor = theme::tone(theme::hsla(0.0, 0.0, 1.0, 1.0), theme::hsla(0.0, 0.0, 0.141, 1.0));
const PILL_HOVER: AdaptiveColor = theme::tone(theme::hsla(0.0, 0.0, 0.969, 1.0), theme::hsla(0.0, 0.0, 0.173, 1.0));
const PILL_BORDER: AdaptiveColor = theme::tone(theme::hsla(0.0, 0.0, 0.0, 0.102), theme::hsla(0.0, 0.0, 1.0, 0.078));

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
        // `TOP` — the controls' strip, level with the traffic lights.
        .h(px(60.0))
        .window_drag(true)
        // The chat scrolls under a fading gradient (App.tsx:2382):
        // `rgba(22,22,22,0.96) → 0.7 @55% → 0`; `tg` swaps the stops to
        // white on light.
        .bg_linear_to_b()
        .gradient_from(theme::tone(
            theme::hsla(0.0, 0.0, 1.0, 0.96),
            theme::hsla(0.0, 0.0, 0.086, 0.96),
        ))
        .gradient_via(theme::tone(
            theme::hsla(0.0, 0.0, 1.0, 0.7),
            theme::hsla(0.0, 0.0, 0.086, 0.7),
        ))
        .gradient_via_position(percentage(0.55))
        .gradient_to(theme::tone(
            theme::hsla(0.0, 0.0, 1.0, 0.0),
            theme::hsla(0.0, 0.0, 0.086, 0.0),
        ))
        .child(
            // Center layer — the pill is `position:absolute` centered in
            // the TSX so side controls never shift it.
            view()
                .absolute()
                .size_full()
                .items_center()
                .justify_center()
                .window_drag(true)
                // Rebuilt on a theme flip: the shadow is a fixed style and
                // differs per theme (`#00000066` dark, `#0000001f` light).
                .child(dynamic(move || {
                    let shadow_a = if dark() { 0.4 } else { 0.12 };
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
                        // Grok: a solid elevated card (not a tint), hairline
                        // border + soft shadow (App.tsx:2411).
                        .bg(move || {
                            if pill_hover.get() {
                                PILL_HOVER
                            } else {
                                PILL
                            }
                        })
                        .border(px(0.5))
                        .border_color(PILL_BORDER)
                        .shadow([BoxShadow {
                            offset_x: px(0.0),
                            offset_y: px(2.0),
                            blur: px(8.0),
                            spread: px(0.0),
                            color: theme::hsla(0.0, 0.0, 0.0, shadow_a).into(),
                            inset: false,
                        }])
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
                        ))
                        .into_element()
                })),
        )
}
