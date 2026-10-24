//! The right detail pane (App.tsx:2465-3007): 320pt of bot profile or,
//! when the open thread has no bot, the app card. Variant A carries the
//! editable avatar (hover → darkened + pencil + the Bot|Generate|Upload
//! editor card), the name field, the model pill and the
//! Details|Library|Computer|Connect tabs; variant B the app's meta,
//! `/state/:app` kvs and the window/pin/publish buttons.

use std::str::FromStr;

use nsh_core::ui::prelude::*;
use nsh_core::ui::View;

use crate::avatar::{bot_avatar, sleeping_blob, BlobEdit, BlobGaze, BotAvatarProps};
use crate::icons::icon;
use crate::live::store;
use crate::store::{AppMeta, Crop, McpServer, Store, RoxApp};
use crate::theme;

/// The pane's top bar height — the TSX's `TOP`.
pub(crate) const TOP: f32 = 60.0;
/// The pane's width (`width: 320`).
const W: f32 = 320.0;
/// The 64pt profile avatar's crop box (`CROP = 84` in the TSX editor).
const CROP: f32 = 84.0;
/// The zoom slider's track (`TRACK = 236`).
const TRACK: f32 = 236.0;

/// The right column — `rightEff`'s panel (App.tsx:2467).
pub(crate) fn pane() -> impl IntoElement {
    let st = store();
    view()
        .w(px(W))
        .h_full()
        .bg(theme::CARD)
        .border_l_1()
        .border_l_color(theme::BORDER_F)
        .child(top_bar())
        .child(
            view().flex_1().child(
                dynamic(move || {
                    if st.active_bot().is_some() {
                        bot_profile().into_element()
                    } else if st.detail_meta.get().is_some() {
                        app_detail().into_element()
                    } else {
                        empty_pane().into_element()
                    }
                }),
            ),
        )
}

/// `avatarEditing`'s floating editor card — mounted at the root layers so
/// its scrim covers the whole window (App.tsx:3163).

/// `pane top` — the window-drag strip's right side: the theme toggle and
/// the close button, both 36pt circles (App.tsx:2474).
fn top_bar() -> impl IntoElement {
    let st = store();
    view()
        .flex_row()
        .items_center()
        .justify_end()
        .gap_2()
        .px_3()
        .h(px(TOP))
        .child(pane_circle_btn("theme", || {
            // App.tsx:2492 — the pref's own icon, not the resolved scheme.
            match store().theme_pref.get().as_str() {
                "light" => "sun",
                "dark" => "moon",
                _ => "systemTheme",
            }
        }, || {
            let st = store();
            st.set_theme(st.next_theme());
        }))
        .child(pane_circle_btn("close", || "close", || {
            store().right_open.set(false);
        }))
        .on_hover_out(|| st.pane_btn_hover.set(String::new()))
}

/// `circleBtn()` — a 36pt border circle hovering to `#262626`.
fn pane_circle_btn(
    key: &'static str,
    name: impl Fn() -> &'static str + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    view()
        .size(px(36.0))
        .rounded(px(18.0))
        .items_center()
        .justify_center()
        .border_1()
        .border_color(theme::BORDER_F)
        .bg(move || {
            if store().pane_btn_hover.get() == key {
                theme::HOVER
            } else {
                theme::CARD2
            }
        })
        .child(dynamic(move || icon_view(name()).into_element()))
        .on_hover_in(move || store().pane_btn_hover.set(key.to_owned()))
        .on_hover_out(|| store().pane_btn_hover.set(String::new()))
        .on_press(on_press)
        .label(match key {
            "theme" => "Theme",
            _ => "Close",
        })
}

fn icon_view(name: &'static str) -> impl IntoElement {
    icon(name)
}

// ————————————————————————— variant A: the bot profile —————————————————————————

/// `bot profile` — the gaze-following 64pt avatar, the name field, the
/// model pill, and the tab pills (App.tsx:2505).
fn bot_profile() -> impl IntoElement {
    let st = store();
    // `panelGaze`/`panelEdit`/`panelEditK` — the pane avatar's own
    // follow/edit drivers (App.tsx:205-207).
    let gaze = BlobGaze::new();
    let edit = BlobEdit::new();
    let edit_k = crate::avatar::FadeValue::new();
    let gaze2 = gaze.clone();
    let gaze3 = gaze.clone();
    let edit2 = edit.clone();
    let edit3 = edit.clone();
    let edit_k2 = edit_k.clone();
    let edit_k3 = edit_k.clone();
    view()
        .flex_1()
        .child(
            // The profile block follows the pointer for the avatar's gaze
            // (`onHoverMove`/`panelGaze`, App.tsx:2513).
            view()
                .items_center()
                .pt_2()
                .on_hover_move(move |e: &HoverEvent| {
                    gaze.follow(e.position.x.0 as f64, e.position.y.0 as f64, 160.0, 40.0, 32.0)
                })
                .on_hover_out(move || gaze3.rest())
                .child(
                    // The 64pt avatar: hover darkens (editK) and shows the
                    // pencil + the "Edit Avatar" tooltip (App.tsx:2519).
                    view()
                        .size(px(64.0))
                        .items_center()
                        .justify_center()
                        .on_hover_in(move || {
                            edit_k.timing(1.0, 220.0);
                            edit.to(1.0);
                            store().avatar_hover.set(true);
                        })
                        .on_hover_out(move || {
                            edit_k2.timing(0.0, 380.0);
                            edit3.to(0.0);
                            store().avatar_hover.set(false);
                        })
                        .on_press(|| store().open_avatar_editor())
                        .label("Edit Bot avatar")
                        .child(
                            dynamic(move || {
                                let Some(b) = st.active_bot() else {
                                    return view().into_element();
                                };
                                let mut p = BotAvatarProps::bot(&b, 80.0);
                                p.box_ = Some(64.0);
                                p.gaze = Some(gaze2.clone());
                                p.edit = Some(edit2.clone());
                                p.morph_shape = true;
                                bot_avatar(p).into_element()
                            })
                            .into_element(),
                        )
                        // The hover pen at `editK` opacity — the avatar
                        // itself darkens via `edit`, no veil (App.tsx:2547).
                        .child(
                            view()
                                .absolute()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .opacity(move || {
                                    if store().avatar_editing.get() {
                                        0.0
                                    } else {
                                        edit_k3.get()
                                    }
                                })
                                .child(
                                    text("✎").text_size(px(22.0)).text_color(white()),
                                ),
                        ),
                )
                // The name field — Enter commits `updateBot {name}`
                // (App.tsx:2560).
                .child(
                    view()
                        .pt_4()
                        .mx(px(40.0))
                        .child(
                            input()
                                // `value={botEditName()}` — the draft is
                                // the text, empty included; the store
                                // reseeds it only when the bot or its
                                // name actually changes.
                                .value(move || store().bot_edit_name.get())
                                .on_change(|v: &str| store().bot_edit_name.set(v.to_owned()))
                                .on_submit(|_: &str| {
                                    let st = store();
                                    let Some(b) = st.active_bot_untracked() else { return };
                                    let n = st.bot_edit_name.get_untracked().trim().to_owned();
                                    if !n.is_empty() && n != b.name {
                                        st.update_bot(&b.id, serde_json::json!({"name": n}));
                                    }
                                })
                                .placeholder("Bot name")
                                .placeholder_color(|| color(theme::GHOST))
                                .text_size(px(17.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_center()
                                .text_color(theme::TEXT)
                                .rounded(px(8.0))
                                .py(px(2.0))
                                .px_2(),
                        ),
                )
                .child(
                    text("Add a label")
                        .text_size(px(11.0))
                        .text_color(theme::MUTED)
                        .pt(px(4.0)),
                )
                .child(model_pill().pt(px(10.0)).into_element())
                // The "Edit Avatar" tooltip — 8pt under the avatar, over
                // the name (App.tsx:2587).
                .child(when(
                    || store().avatar_hover.get() && !store().avatar_editing.get(),
                    || {
                        view()
                            .absolute()
                            .top(px(80.0))
                            .py(px(4.0))
                            .px(px(6.0))
                            .rounded(px(6.0))
                            .bg(theme::CARD2)
                            .border_1()
                            .border_color(theme::BORDER_F)
                            .child(text("Edit Avatar").text_size(px(12.0)).text_color(theme::TEXT))
                            .into_element()
                    },
                ))
                .child(tab_pills()),
        )
        .child(
            scroll()
                .flex_1()
                .child(
                    view()
                        .px_4()
                        .pt(px(10.0))
                        .pb_6()
                        .child(dynamic(|| detail_body().into_element())),
                ),
        )
}

/// `ModelPill(choice)` — the shared pill (`models::model_pill`,
/// modelpicker.tsx:60): the bot's model + effort label, opening the
/// anchored popover.
fn model_pill() -> View {
    view().child(crate::models::model_pill())
}

/// The Details|Library|Computer|Connect pills (App.tsx:2605).
fn tab_pills() -> impl IntoElement {
    let tabs = ["details", "library", "computer", "connect"];
    view()
        .flex_row()
        .gap(px(1.0))
        .pt(px(14.0))
        .pb(px(10.0))
        .children(tabs.into_iter().map(|tb| {
            view()
                .rounded(px(999.0))
                .px_2()
                .py_1()
                .bg(move || {
                    if store().detail_tab.get() == tb {
                        theme::SUBTLE
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .child(
                    text({
                        let mut c = tb.chars();
                        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                    })
                    .text_size(px(13.0))
                    .text_color(move || {
                        if store().detail_tab.get() == tb {
                            theme::TEXT
                        } else {
                            theme::DIM
                        }
                    }),
                )
                .on_press(move || {
                    let st = store();
                    st.detail_tab.set(tb.to_owned());
                    if tb == "connect" {
                        st.fetch_mcp();
                    }
                })
                .into_element()
        }))
}

/// The tab's body — one of the four cards (App.tsx:2626).
fn detail_body() -> impl IntoElement {
    match store().detail_tab.get().as_str() {
        "library" => library_tab().into_element(),
        "computer" => computer_tab().into_element(),
        "connect" => connect_tab().into_element(),
        _ => details_tab().into_element(),
    }
}

/// `paneLabel` — a section's 13pt dim title (App.tsx:1233).
fn pane_label(t: String) -> impl IntoElement {
    text(t)
        .text_size(px(13.0))
        .text_color(theme::DIM)
        .px_3()
        .pb(px(10.0))
}

/// `paneCard()` — the `#1f1f1f` 12pt card with a 0.5pt border.
fn pane_card() -> View {
    view()
        .bg(theme::CARD2)
        .rounded(px(12.0))
        .border_1()
        .border_color(theme::BORDER_F)
        .p_1()
}

/// Details — the Routines card (App.tsx:2627).
fn details_tab() -> impl IntoElement {
    view()
        .child(pane_label("Routines".into()))
        .child(
            pane_card()
                .py(px(14.0))
                .px(px(13.0))
                .child(
                    text(move || {
                        let name = store().active_bot().map(|b| b.name).unwrap_or_default();
                        format!("No routines yet. Ask {name} in chat to run something on a schedule.")
                    })
                    .text_size(px(13.0))
                    .text_color(theme::DIM),
                ),
        )
}

/// Library — `libraryGroups()` + version trees (App.tsx:2637).
fn library_tab() -> impl IntoElement {
    view()
        .child(when(
            || store().thread_apps.get().is_empty(),
            || {
                view()
                    .child(pane_label("Library".into()))
                    .child(
                        pane_card()
                            .py(px(14.0))
                            .px(px(13.0))
                            .child(
                                text(move || {
                                    let name = store()
                                        .active_bot()
                                        .map(|b| b.name)
                                        .unwrap_or_default();
                                    format!("Files {name} makes show up here.")
                                })
                                .text_size(px(13.0))
                                .text_color(theme::DIM),
                            ),
                    )
                    .into_element()
            },
        ))
        .child(each(
            || store().library_groups(),
            |g| g.0.clone(),
            |g, _| {
                // The group's rows come from a second `each` on the live
                // signal: the outer key is the label, so an agent switch
                // that lands in the same buckets ("Older" → "Older")
                // would keep the first agent's `items` forever.
                let label = g.get_untracked().0;
                view()
                    .pb(px(22.0))
                    .child(pane_label(label))
                    .child(
                        pane_card().child(each(
                            move || g.get().1,
                            |a| a.app.clone(),
                            |a, _| app_row(a).into_element(),
                        )),
                    )
                    .into_element()
            },
        ))
}

/// A library row — the icon tile, title/file, `v<n>` chip, `ago` (App.tsx:2648).
fn app_row(a: ReadSignal<RoxApp>) -> impl IntoElement {
    let a0 = a.get();
    let name = a0.app.clone();
    let tile_hue = hue(&a0.app);
    let n_chip = a0.app.clone();
    let a_ctx = a.clone();
    let a_tree = a.clone();
    view()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(7.0))
                .py(px(6.0))
                .px_2()
                .rounded(px(8.0))
                .on_press(move || store().popout(&name))
                .on_context_menu(move |e: &ContextMenuEvent| {
                    crate::ctx::open(e, crate::ctx::app_items(&a_ctx.get()))
                })
                .child(
                    // `hue(name)` tile — `</>` on the deterministic color.
                    view()
                        .size(px(24.0))
                        .rounded(px(5.0))
                        .items_center()
                        .justify_center()
                        .bg(tile_hue)
                        .child(
                            text("</>")
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(white()),
                        ),
                )
                .child(
                    view()
                        .flex_1()
                        .child(
                            text({
                                let a = a.clone();
                                move || a.get().title
                            })
                            .text_size(px(13.0))
                            .text_color(theme::TEXT)
                            .number_of_lines(1),
                        )
                        .child(
                            text({
                                let a = a.clone();
                                move || a.get().file
                            })
                            .text_size(px(12.0))
                            .text_color(theme::MUTED)
                            .number_of_lines(1),
                        ),
                )
                .child(when(
                    {
                        let a = a.clone();
                        move || a.get().v > 0
                    },
                    move || {
                        let n = n_chip.clone();
                        let a2 = a.clone();
                        view()
                            .rounded(px(6.0))
                            .px(px(6.0))
                            .py(px(2.0))
                            .bg(theme::HOVER)
                            .child(
                                text(move || format!("v{} ⎇", a2.get().v))
                                    .text_size(px(11.0))
                                    .font_family("monospace")
                                    .text_color(theme::TEXT),
                            )
                            .on_press(move || {
                                let st = store();
                                st.vers_open.set(if st.vers_open.get_untracked() == n {
                                    String::new()
                                } else {
                                    n.clone()
                                });
                            })
                            .into_element()
                    },
                ))
                .child(
                    text(move || Store::ago(a.get().mtime))
                        .text_size(px(13.0))
                        .text_color(theme::MUTED),
                ),
        )
        .child(version_tree(a_tree))
}

/// The `v<n> ⎇` tree — newest first; a click moves the pointer
/// (App.tsx:2689).
fn version_tree(a: ReadSignal<RoxApp>) -> impl IntoElement {
    let a0 = a.get();
    let n = a0.app.clone();
    let name = a0.app.clone();
    let open = move || store().vers_open.get() == n && !a.get().versions.is_empty();
    when(open, move || {
        let cur = a.get();
        let cur_v = cur.v;
        let mut versions = cur.versions.clone();
        versions.sort_by(|x, y| y.v.cmp(&x.v));
        view()
            .pl_3()
            .py(px(2.0))
            .children(versions.iter().map(|vr| {
                let cur = vr.v == cur_v;
                let v = vr.v;
                let v2 = vr.v;
                let created = vr.created_at;
                let name = name.clone();
                let name2 = name.clone();
                let label = vr.label.clone();
                let label2 = label.clone();
                let parent = vr.parent;
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(7.0))
                    .py(px(4.0))
                    .px_2()
                    .on_press(move || store().set_version(name.clone(), v))
                    .child(
                        text(if cur { "●" } else { "○" })
                            .text_size(px(11.0))
                            .text_color(if cur {
                                Hsla::from_str("#32bb78").unwrap_or(theme::ACCENT.dark)
                            } else {
                                theme::GHOST.dark
                            }),
                    )
                    .child(
                        text(format!("v{v2}"))
                            .text_size(px(12.0))
                            .font_family("monospace")
                            .text_color(if cur { theme::TEXT.dark } else { theme::MUTED.dark }),
                    )
                    .children(if label.is_empty() {
                        None
                    } else {
                        Some(
                            text(label.clone())
                                .text_size(px(11.0))
                                .text_color(Hsla::from_str("#c4b0ff").unwrap_or(theme::TEXT.dark))
                                .max_w(px(90.0))
                                .number_of_lines(1)
                                .into_element(),
                        )
                    })
                    .children(if parent.is_some() && parent != Some(v2 - 1) {
                        Some(
                            text(format!("⎇ from v{}", parent.unwrap_or(0)))
                                .text_size(px(11.0))
                                .text_color(Hsla::from_str("#b16dff").unwrap_or(theme::TEXT.dark))
                                .into_element(),
                        )
                    } else {
                        None
                    })
                    .child(
                        view()
                            .rounded(px(5.0))
                            .px(px(3.0))
                            .child(icon("edit"))
                            .on_press(move || {
                                let st = store();
                                st.app_rename_text.set(label2.clone());
                                st.app_rename.set(Some(crate::store::AppRename {
                                    name: name2.clone(),
                                    field: "label".into(),
                                    v: Some(v2),
                                }));
                            }),
                    )
                    .children(if cur {
                        Some(
                            text("current")
                                .text_size(px(11.0))
                                .text_color(Hsla::from_str("#32bb78").unwrap_or(theme::ACCENT.dark))
                                .into_element(),
                        )
                    } else {
                        None
                    })
                    .child(
                        text(Store::ago(created))
                            .text_size(px(11.0))
                            .text_color(theme::GHOST)
                            .flex_1(),
                    )
                    .into_element()
            }))
            .into_element()
    })
}

/// Computer — the sleeping avatar card (App.tsx:2814).
fn computer_tab() -> impl IntoElement {
    view()
        .child(
            view()
                .h(px(180.0))
                .rounded(px(12.0))
                .bg(theme::MAIN)
                .items_center()
                .justify_center()
                .label("Wake computer")
                .child(
                    dynamic(|| {
                        let shape = store().active_bot().map(|b| b.shape).unwrap_or_else(|| "blob".into());
                        sleeping_blob(&shape, 33.0, "#f5f5f58a").into_element()
                    })
                    .into_element(),
                )
                .child(
                    text("Computer is asleep")
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::TEXT)
                        .pt(px(6.0)),
                ),
        )
        .child(
            text(move || {
                let name = store().active_bot().map(|b| b.name).unwrap_or_default();
                format!("{name}'s screen")
            })
            .text_size(px(12.0))
            .text_color(theme::MUTED)
            .pt(px(10.0)),
        )
}

/// Connect — the MCP servers card + the "Connect app" form (App.tsx:2735).
fn connect_tab() -> impl IntoElement {
    let st = store();
    if st.mcp_servers.get_untracked().is_empty() && !st.mcp_form_open.get_untracked() {
        st.fetch_mcp();
    }
    view()
        .child(pane_label("Connected apps".into()))
        .child(
            pane_card()
                .child(when(
                    || store().mcp_servers.get().is_empty(),
                    || {
                        let name = store().active_bot().map(|b| b.name).unwrap_or_default();
                        text(format!("No apps connected yet. Add an MCP server below — its tools become available to {name}."))
                            .text_size(px(13.0))
                            .text_color(theme::DIM)
                            .py(px(10.0))
                            .px(px(13.0))
                            .into_element()
                    },
                ))
                .child(each(
                    || store().mcp_servers.get(),
                    |s| s.id.clone(),
                    |s, _| mcp_row(s.get()),
                )),
        )
        .child(mcp_form_card().mt(px(10.0)))
}

/// An MCP server row — status dot, name + GLOBAL, `tools · command args`.
fn mcp_row(s: McpServer) -> impl IntoElement {
    let s2 = s.clone();
    let id_hue = hue(&s.id);
    let name = s.name.clone();
    let bot = s.bot.clone();
    let cmd = format!("{} {}", s.command, s.args.join(" ")).trim().to_owned();
    let tools = s.tools;
    let err = s.err.clone();
    let status = s.status.clone();
    let subtitle = match status.as_str() {
        "connected" => format!("{} tools · {}", tools, cmd),
        "error" => format!("error — {}", if err.is_empty() { "failed to start".to_owned() } else { err }),
        "off" => format!("off · {}", cmd),
        _ => cmd,
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(9.0))
        .py(px(8.0))
        .px(px(10.0))
        .on_context_menu(move |e: &ContextMenuEvent| {
            crate::ctx::open(e, crate::ctx::mcp_items(&s2))
        })
        .child(
            view()
                .size(px(24.0))
                .rounded(px(6.0))
                .items_center()
                .justify_center()
                .bg(id_hue)
                .child(
                    text("⬡")
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(white()),
                ),
        )
        .child(
            view()
                .flex_1()
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            text(name)
                                .text_size(px(13.0))
                                .text_color(theme::TEXT)
                                .number_of_lines(1),
                        )
                        .child(when(
                            move || bot.is_empty(),
                            || {
                                text("GLOBAL")
                                    .text_size(px(9.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(Hsla::from_str("#8fb6ff").unwrap_or(theme::ACCENT.dark))
                                    .into_element()
                            },
                        )),
                )
                .child(
                    text(subtitle)
                        .text_size(px(11.0))
                        .text_color(theme::MUTED)
                        .number_of_lines(1),
                ),
        )
        .child(
            view()
                .size(px(8.0))
                .rounded(px(4.0))
                .bg(match status.as_str() {
                    "connected" => Hsla::from_str("#32bb78").unwrap_or(theme::ACCENT.dark),
                    "error" => Hsla::from_str("#ff5f57").unwrap_or(theme::TEXT.dark),
                    "off" => theme::GHOST.dark,
                    _ => Hsla::from_str("#febc2e").unwrap_or(theme::TEXT.dark),
                }),
        )
}

/// The "Connect app" form — four fields, the scope pills, Connect
/// (App.tsx:2775).
fn mcp_form_card() -> View {
    view()
        .bg(theme::CARD2)
        .rounded(px(12.0))
        .border_1()
        .border_color(theme::BORDER_F)
        .py(px(10.0))
        .px_3()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .on_press(|| store().mcp_form_open.update(|v| *v = !*v))
                .child(
                    text(move || {
                        if store().mcp_form_open.get() { "− Cancel".to_owned() } else { "＋ Connect app".to_owned() }
                    })
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT),
                )
                .child(
                    text(move || {
                        if store().mcp_form_open.get() { String::new() } else { "stdio MCP server".into() }
                    })
                    .text_size(px(11.0))
                    .text_color(theme::DIM),
                ),
        )
        .child(when(
            || store().mcp_form_open.get(),
            || {
                view()
                    .pt_2()
                    .children(
                        [("name", "Name (e.g. Linear)"), ("command", "Command (e.g. npx)"), ("args", "Args (e.g. -y @modelcontextprotocol/server-…)"), ("env", "Env (KEY=V; KEY2=V2) — optional")]
                            .into_iter()
                            .map(|(f, ph)| {
                                view()
                                    .rounded(px(8.0))
                                    .border_1()
                                    .border_color(theme::BORDER_S)
                                    .bg(theme::MAIN)
                                    .px(px(9.0))
                                    .child(
                                        input()
                                            .value(move || {
                                                let form = store().mcp_form.get();
                                                match f {
                                                    "name" => form.name,
                                                    "command" => form.command,
                                                    "args" => form.args,
                                                    _ => form.env,
                                                }
                                            })
                                            .on_change(move |v: &str| {
                                                store().mcp_form.update(|form| match f {
                                                    "name" => form.name = v.to_owned(),
                                                    "command" => form.command = v.to_owned(),
                                                    "args" => form.args = v.to_owned(),
                                                    _ => form.env = v.to_owned(),
                                                })
                                            })
                                            .placeholder(ph)
                                            .placeholder_color(|| color(theme::GHOST))
                                            .text_size(px(12.0))
                                            .text_color(theme::TEXT)
                                            .py(px(7.0)),
                                    )
                                    .mt_2()
                                    .into_element()
                            }),
                    )
                    .child(
                        // `scope` — all bots vs this one (App.tsx:2800).
                        view()
                            .flex_row()
                            .gap(px(6.0))
                            .mt_2()
                            .children(["all", "bot"].into_iter().map(|sc| {
                                view()
                                    .rounded(px(999.0))
                                    .px(px(10.0))
                                    .py(px(5.0))
                                    .border_1()
                                    .border_color(move || {
                                        if store().mcp_form.get().scope == sc {
                                            theme::BORDER_S
                                        } else {
                                            theme::BORDER_F
                                        }
                                    })
                                    .bg(move || {
                                        if store().mcp_form.get().scope == sc {
                                            theme::HOVER
                                        } else {
                                            AdaptiveColor::TRANSPARENT
                                        }
                                    })
                                    .child(
                                        text(if sc == "all" {
                                            "All bots".to_owned()
                                        } else {
                                            let n = store().active_bot().map(|b| b.name).unwrap_or_default();
                                            format!("{n} only")
                                        })
                                        .text_size(px(11.0))
                                        .text_color(move || {
                                            if store().mcp_form.get().scope == sc {
                                                theme::TEXT
                                            } else {
                                                theme::DIM
                                            }
                                        }),
                                    )
                                    .on_press(move || {
                                        store().mcp_form.update(|f| f.scope = sc.to_owned())
                                    })
                                    .into_element()
                            })),
                    )
                    .child(when(
                        || !store().mcp_err.get().is_empty(),
                        || {
                            text(|| store().mcp_err.get())
                                .text_size(px(11.0))
                                .text_color(Hsla::from_str("#ff8f89").unwrap_or(theme::TEXT.dark))
                                .mt_2()
                                .into_element()
                        },
                    ))
                    .child(
                        view()
                            .mt_2()
                            .self_start()
                            .rounded(px(999.0))
                            .bg(theme::TEXT)
                            .px(px(14.0))
                            .py(px(7.0))
                            .child(
                                text(move || {
                                    if store().mcp_busy.get() { "Connecting…" } else { "Connect" }
                                })
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::CARD),
                            )
                            .on_press(|| store().add_mcp_server()),
                    )
                    .into_element()
            },
        ))
}

// ————————————————————————— variant B: the app card —————————————————————————

/// `detail() && !activeBot()` — the app detail: tile, title, pills,
/// kvs, actions (App.tsx:2845).
fn app_detail() -> impl IntoElement {
    view()
        .flex_1()
        .items_center()
        .px(px(14.0))
        .child(
            view()
                .size(px(64.0))
                .rounded(px(20.0))
                .items_center()
                .justify_center()
                .bg(move || {
                    store()
                        .detail_meta
                        .get()
                        .map(|m| hue(&m.name).dark)
                        .unwrap_or(theme::CARD2.dark)
                })
                .child(
                    text(move || {
                        store()
                            .detail_meta
                            .get()
                            .map(|m| {
                                m.title
                                    .chars()
                                    .next()
                                    .map(|c| c.to_uppercase().collect::<String>())
                                    .unwrap_or_else(|| "?".into())
                            })
                            .unwrap_or_else(|| "?".into())
                    })
                    .text_size(px(26.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(white()),
                ),
        )
        .child(
            text(move || {
                store().detail_meta.get().map(|m| m.title).unwrap_or_default()
            })
            .text_size(px(15.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::TEXT)
            .pt(px(10.0)),
        )
        .child(
            text(move || {
                store()
                    .detail_meta
                    .get()
                    .map(|m| {
                        format!(
                            "app · {}.tsx{}",
                            m.name,
                            if m.pinned { " · ★ pinned" } else { "" }
                        )
                    })
                    .unwrap_or_default()
            })
            .text_size(px(11.0))
            .text_color(theme::GHOST),
        )
        .child(detail_pills().mt_1().into_element())
        .child(
            scroll()
                .flex_1()
                .child(
                    view()
                        .pt_3()
                        .child(dynamic(|| detail_variant_body().into_element())),
                ),
        )
        .child(app_actions().pb_1().into_element())
}

/// The Details|State segmented pills (App.tsx:2872).
fn detail_pills() -> View {
    view()
        .flex_row()
        .bg(theme::HOVER)
        .rounded(px(999.0))
        .p(px(3.0))
        .children(["details", "state"].into_iter().map(|tb| {
            view()
                .rounded(px(999.0))
                .px_4()
                .py_1()
                .bg(move || {
                    if store().detail_tab.get() == tb {
                        theme::ACTIVE
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .child(
                    text(if tb == "details" { "Details" } else { "State" })
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || {
                            if store().detail_tab.get() == tb {
                                theme::TEXT
                            } else {
                                theme::MUTED
                            }
                        }),
                )
                .on_press(move || store().detail_tab.set(tb.to_owned()))
                .into_element()
        }))
}

/// Variant B's scroll body — meta rows or the state kvs.
fn detail_variant_body() -> impl IntoElement {
    if store().detail_tab.get() == "state" {
        view()
            .gap_2()
            .child(when(
                || store().detail.get().is_empty(),
                || {
                    text("no persisted state")
                        .text_size(px(12.0))
                        .text_color(theme::GHOST)
                        .into_element()
                },
            ))
            .child(each(
                || store().detail.get(),
                |kv| kv.0.clone(),
                |kv, _| state_row(kv.get()),
            ))
            .into_element()
    } else {
        view()
            .gap_2()
            .child(meta_row("module", |m| m.name.clone()))
            .child(meta_row("status", |m| {
                if m.published { "in Mini Apps".into() } else { "chat only".into() }
            }))
            .child(when(
                || !store().detail_meta.get().map(|m| m.kinds.is_empty()).unwrap_or(true),
                || meta_row("kinds", |m| m.kinds.join(" · ")).into_element(),
            ))
            .child(when(
                || !store().detail_meta.get().map(|m| m.tags.is_empty()).unwrap_or(true),
                || {
                    meta_row("tags", |m| {
                        m.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ")
                    })
                    .into_element()
                },
            ))
            .into_element()
    }
}

fn meta_row(label: &'static str, value: impl Fn(&AppMeta) -> String + 'static) -> impl IntoElement {
    view()
        .flex_row()
        .justify_between()
        .child(text(label).text_size(px(12.0)).text_color(theme::MUTED))
        .child(
            text(move || {
                store()
                    .detail_meta
                    .get()
                    .map(|m| value(&m))
                    .unwrap_or_default()
            })
            .text_size(px(12.0))
            .text_color(theme::TEXT),
        )
}

fn state_row(kv: crate::store::Kv) -> impl IntoElement {
    view()
        .flex_row()
        .justify_between()
        .gap_2()
        .child(
            text(kv.0)
                .text_size(px(12.0))
                .text_color(theme::MUTED),
        )
        .child(
            text(kv.1)
                .text_size(px(12.0))
                .font_family("Menlo")
                .text_color(theme::TEXT)
                .number_of_lines(2),
        )
}

/// The action buttons — `Open window`, pin/unpin, `Save to Mini Apps`
/// (App.tsx:2963).
fn app_actions() -> View {
    view()
        .gap_2()
        .child(
            action_btn("Open window", false, || {
                let Some(m) = store().detail_meta.get_untracked() else { return };
                store().popout(&m.name);
            }),
        )
        .child(
            action_btn_dyn(
                || {
                    if store().detail_meta.get().map(|m| m.pinned).unwrap_or(false) {
                        "Unpin from desktop widget"
                    } else {
                        "Pin to desktop widget"
                    }
                },
                false,
                || {
                    let Some(m) = store().detail_meta.get_untracked() else { return };
                    let st = store();
                    spawn(async move {
                        let _ = client_post("/pin", serde_json::json!({"name": m.name, "pinned": !m.pinned})).await;
                        st.refresh_threads();
                    });
                },
            ),
        )
        .child(when(
            || !store().detail_meta.get().map(|m| m.published).unwrap_or(true),
            || {
                action_btn("Save to Mini Apps", true, || {
                    let Some(m) = store().detail_meta.get_untracked() else { return };
                    let st = store();
                    spawn(async move {
                        let _ = client_post("/publish", serde_json::json!({"name": m.name})).await;
                        st.refresh_threads();
                    });
                })
                .into_element()
            },
        ))
}

fn action_btn(label: &'static str, accent: bool, on_press: impl Fn() + 'static) -> impl IntoElement {
    view()
        .rounded(px(10.0))
        .bg(if accent {
            Hsla::from_str("#1884fc").unwrap_or(theme::ACCENT.dark)
        } else {
            theme::HOVER.dark
        })
        .py(px(9.0))
        .items_center()
        .child(
            text(label)
                .text_size(px(13.0))
                .font_weight(if accent { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                .text_color(if accent { white() } else { color(theme::TEXT) }),
        )
        .on_press(on_press)
}

fn action_btn_dyn(
    label: impl Fn() -> &'static str + 'static,
    accent: bool,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    view()
        .rounded(px(10.0))
        .bg(if accent {
            Hsla::from_str("#1884fc").unwrap_or(theme::ACCENT.dark)
        } else {
            theme::HOVER.dark
        })
        .py(px(9.0))
        .items_center()
        .child(
            text(label)
                .text_size(px(13.0))
                .font_weight(if accent { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                .text_color(if accent { white() } else { color(theme::TEXT) }),
        )
        .on_press(on_press)
}

/// The empty pane — no bot and no app yet (App.tsx:2835).
fn empty_pane() -> impl IntoElement {
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_2()
        .px(px(14.0))
        .child(
            text("No app in this conversation yet")
                .text_size(px(13.0))
                .text_color(theme::GHOST),
        )
        .child(
            text("generated apps show their details here")
                .text_size(px(11.0))
                .text_color(theme::GHOST),
        )
}

// ————————————————————————— the avatar editor —————————————————————————

/// `avatarEditing`'s floating card under the pane avatar — 297 wide,
/// Bot | Generate | Upload tabs (App.tsx:3163). Rendered from `pane()`.
pub(crate) fn editor_overlay() -> impl IntoElement {
    dynamic(|| {
        let st = store();
        if !(st.avatar_editing.get() && st.right_open.get() && st.active_bot().is_some()) {
            return view().into_element();
        }
        view()
            .size_full()
            .absolute()
            // The outside-tap scrim — `onPress={closeAvatarEditor}`.
            .child(
                view()
                    .size_full()
                    .on_press(|| store().close_avatar_editor()),
            )
            .child(
                view()
                    .absolute()
                    .right(px(11.0))
                    .top(px(TOP + 8.0 + 64.0 + 6.0))
                    .w(px(297.0))
                    .rounded(px(16.0))
                    .bg(theme::EDITOR)
                    .border_1()
                    .border_color(theme::BORDER_F)
                    .child(editor_tabs())
                    .child(dynamic(|| editor_body().into_element())),
            )
            .into_element()
    })
}

/// The Bot|Generate|Upload pills + Reset (App.tsx:3181).
fn editor_tabs() -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .pt(px(9.0))
        .px_2()
        .gap(px(2.0))
        .children(["bot", "generate", "upload"].into_iter().map(|tb| {
            view()
                .rounded(px(999.0))
                .px_2()
                .py_1()
                .bg(move || {
                    if store().avatar_edit_tab.get() == tb {
                        theme::SUBTLE
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .child(
                    text({
                        let mut c = tb.chars();
                        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                    })
                    .text_size(px(13.0))
                    .text_color(move || {
                        if store().avatar_edit_tab.get() == tb {
                            theme::TEXT
                        } else {
                            theme::DIM
                        }
                    }),
                )
                .on_press(move || {
                    let st = store();
                    st.avatar_edit_tab.set(tb.to_owned());
                    st.avatar_err.set(String::new());
                })
                .into_element()
        }))
        .child(view().flex_1())
        .child(when(
            || store().avatar_edit_tab.get() == "bot",
            || {
                view()
                    .h(px(22.0))
                    .px_2()
                    .rounded(px(8.0))
                    .justify_center()
                    .label("Reset avatar")
                    .child(text("Reset").text_size(px(13.0)).text_color(theme::DIM))
                    .on_press(|| {
                        let Some(b) = store().active_bot_untracked() else { return };
                        store().update_bot(
                            &b.id,
                            serde_json::json!({"shape": "blob", "color": "#FFFFFF", "image": "", "crop": serde_json::Value::Null}),
                        );
                    })
                    .into_element()
            },
        ))
}

/// The editor's body — the bot grid, the generate prompt, or the upload
/// zone (App.tsx:3214).
fn editor_body() -> impl IntoElement {
    match store().avatar_edit_tab.get().as_str() {
        "generate" => generate_tab().into_element(),
        "upload" => upload_tab().into_element(),
        _ => bot_tab().into_element(),
    }
}

/// `Bot` — 48pt shapes five per row, 32pt swatches six per row
/// (App.tsx:3215).
fn bot_tab() -> impl IntoElement {
    let st = store();
    view()
        .pt(px(26.0))
        .px_3()
        .pb(px(20.0))
        .gap(px(26.0))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .justify_center()
                .gap_2()
                .children(crate::mark::BOT_SHAPES.iter().map(|sh| {
                    let sh = *sh;
                    view()
                        .size(px(48.0))
                        .items_center()
                        .justify_center()
                        .label(format!("{sh} character shape"))
                        .on_press(move || {
                            let Some(b) = st.active_bot_untracked() else { return };
                            st.update_bot(
                                &b.id,
                                serde_json::json!({"shape": sh, "image": "", "crop": serde_json::Value::Null}),
                            );
                        })
                        .child(when(
                            move || {
                                store()
                                    .active_bot()
                                    .map(|b| b.image.is_empty() && b.shape == sh)
                                    .unwrap_or(false)
                            },
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
                        // App.tsx:3248 — the adaptive swatch raw; the
                        // painter resolves `inkOf` per theme.
                        .child(crate::avatar::bot_blob(crate::avatar::BotBlobProps {
                            shape: sh.into(),
                            color: "#FFFFFF".to_owned(),
                            size: 44.0,
                            paused: true,
                            ..Default::default()
                        }))
                        .into_element()
                })),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .justify_center()
                .gap_2()
                .px_5()
                .children(crate::mark::BOT_COLORS.iter().map(|c| {
                    let c = *c;
                    view()
                        .size(px(32.0))
                        .rounded(px(16.0))
                        .items_center()
                        .justify_center()
                        .border_1()
                        .border_color(move || {
                            let same = store()
                                .active_bot()
                                .map(|b| {
                                    b.image.is_empty()
                                        && crate::mark::norm_color(&b.color) == crate::mark::norm_color(c)
                                })
                                .unwrap_or(false);
                            if same { theme::BORDER_S } else { AdaptiveColor::TRANSPARENT }
                        })
                        .label(format!("{c} character color"))
                        .on_press(move || {
                            let Some(b) = store().active_bot_untracked() else { return };
                            store().update_bot(
                                &b.id,
                                serde_json::json!({"color": c, "image": "", "crop": serde_json::Value::Null}),
                            );
                        })
                        .child(
                            view()
                                .size(px(24.0))
                                .rounded(px(12.0))
                                // `inkOf(c())` on a style binding — follows
                                // the env memo when the theme flips.
                                .bg(move || {
                                    Hsla::from_str(&crate::mark::ink_of(c, !dark()))
                                        .unwrap_or_else(|_| theme::TEXT.dark)
                                }),
                        )
                        .into_element()
                })),
        )
}

/// `Generate` — the prompt, the pulsing placeholder, or the crop
/// (App.tsx:3261).
fn generate_tab() -> impl IntoElement {
    view()
        .pt(px(20.0))
        .px_3()
        .pb(px(13.0))
        .gap_3()
        .child(when(
            || store().avatar_draft.get().is_none(),
            || {
                view()
                    .gap_3()
                    .child(
                        input()
                            .value(|| store().avatar_gen_prompt.get())
                            .on_change(|v: &str| store().avatar_gen_prompt.set(v.to_owned()))
                            .placeholder("Describe your avatar…")
                            .placeholder_color(|| color(theme::GHOST))
                            .multiline(|| !store().avatar_gen_busy.get())
                            .readonly(|| store().avatar_gen_busy.get())
                            .h(move || px(if store().avatar_gen_busy.get() { 32.0 } else { 174.0 }))
                            .text_size(px(14.0))
                            .text_color(theme::TEXT)
                            .bg(theme::CARD2)
                            .py_2()
                            .px(px(10.0))
                            .rounded(px(10.0))
                            .border_1()
                            .border_color(theme::BORDER_S),
                    )
                    .child(when(
                        || store().avatar_gen_busy.get(),
                        || {
                            view()
                                .items_center()
                                .py_3()
                                .child(
                                    view()
                                        .size(px(84.0))
                                        .rounded(px(42.0))
                                        .bg(Hsla::from_str("#2a2a2a").unwrap_or(theme::CARD2.dark)),
                                )
                                .into_element()
                        },
                    ))
                    .child(
                        view()
                            .h(px(30.0))
                            .rounded(px(10.0))
                            .items_center()
                            .justify_center()
                            .bg(move || {
                                if store().avatar_gen_busy.get() {
                                    theme::HOVER.dark
                                } else if !store().avatar_gen_prompt.get().trim().is_empty() {
                                    theme::TEXT.dark
                                } else {
                                    theme::GHOST.dark
                                }
                            })
                            .child(
                                text(move || {
                                    if store().avatar_gen_busy.get() { "Generating…" } else { "Generate" }
                                })
                                .text_size(px(13.0))
                                .text_color(move || {
                                    if store().avatar_gen_busy.get() {
                                        theme::MUTED.dark
                                    } else {
                                        theme::CARD2.dark
                                    }
                                }),
                            )
                            .on_press(|| store().generate_avatar()),
                    )
                    .into_element()
            },
        ))
        .child(when(
            || store().avatar_draft.get().is_some(),
            || {
                let d = store().avatar_draft.get().unwrap_or_default();
                crop_editor(d, false).into_element()
            },
        ))
        .child(editor_err())
}

/// `Upload` — the dashed drop zone, then the crop with name·size
/// (App.tsx:3328).
fn upload_tab() -> impl IntoElement {
    view()
        .pt(px(20.0))
        .px_3()
        .pb(px(13.0))
        .gap_3()
        .child(when(
            || store().avatar_draft.get().is_none(),
            || {
                view()
                    .h(px(216.0))
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(theme::BORDER_S)
                    .items_center()
                    .justify_center()
                    .px_3()
                    .child(
                        text("Drag, drop, or paste an image")
                            .text_size(px(14.0))
                            .text_color(theme::DIM),
                    )
                    .child(text("or").text_size(px(14.0)).text_color(theme::MUTED).mt_2())
                    .child(
                        view()
                            .mt_2()
                            .h(px(30.0))
                            .px(px(13.0))
                            .rounded(px(10.0))
                            .bg(theme::SUBTLE)
                            .justify_center()
                            .child(text("Browse files").text_size(px(13.0)).text_color(theme::TEXT))
                            .on_press(|| store().pick_avatar("pick")),
                    )
                    .into_element()
            },
        ))
        .child(when(
            || store().avatar_draft.get().is_some(),
            || {
                let d = store().avatar_draft.get().unwrap_or_default();
                crop_editor(d, true).into_element()
            },
        ))
        .child(editor_err())
}

fn editor_err() -> impl IntoElement {
    when(
        || !store().avatar_err.get().is_empty(),
        || {
            text(|| store().avatar_err.get())
                .text_size(px(12.0))
                .text_color(Hsla::from_str("#ff7b7b").unwrap_or(theme::TEXT.dark))
                .into_element()
        },
    )
}

/// `cropEditor` — the round image, the zoom slider, Restart / Set avatar
/// (App.tsx:1744).
fn crop_editor(d: crate::store::AvatarDraft, show_name: bool) -> impl IntoElement {
    // `dragFrom` — the crop when the pan started (`onPanResponderGrant`).
    let drag_from = std::rc::Rc::new(std::cell::Cell::new(Crop { s: 1.0, x: 0.0, y: 0.0 }));
    let df1 = drag_from.clone();
    let df2 = drag_from.clone();
    let df3 = drag_from.clone();
    let df4 = drag_from.clone();
    let uri = d.uri.clone();
    view()
        .gap_3()
        .child(
            view()
                .items_center()
                .gap(px(6.0))
                .child(
                    view()
                        .on_pan_start(move |_: &PanEvent| {
                            df1.set(store().draft_crop.get_untracked());
                        })
                        .on_pan_end(move |e: &PanEvent| {
                            let f = df2.get();
                            store().crop_drag(f, e.translation.x.0 as f64, e.translation.y.0 as f64);
                        })
                        .child(dynamic(move || {
                            crate::avatar::cropped_image(
                                &uri,
                                CROP,
                                Some(store().draft_crop.get()),
                            )
                            .into_element()
                        })),
                )
                .child(when(
                    move || show_name,
                    {
                        let d = d.clone();
                        move || {
                            text(format!("{} · {}×{}", d.name, d.w as i64, d.h as i64))
                                .text_size(px(13.0))
                                .text_color(theme::DIM)
                                .px_2()
                                .mt(px(6.0))
                                .into_element()
                        }
                    },
                ))
                .child(
                    text("Drag to reposition")
                        .text_size(px(13.0))
                        .text_color(theme::MUTED),
                ),
        )
        .child(
            // The 1×–3× zoom slider: `−`/`+` step 0.25, the track pans
            // `s` (App.tsx:1766).
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    view()
                        .w(px(14.0))
                        .items_center()
                        .label("Zoom out")
                        .child(text("−").text_size(px(15.0)).text_color(theme::DIM))
                        .on_press(|| store().crop_zoom(-0.25)),
                )
                .child(
                    view()
                        .w(px(TRACK))
                        .h(px(20.0))
                        .justify_center()
                        .on_pan_start(move |_: &PanEvent| {
                            df3.set(store().draft_crop.get_untracked());
                        })
                        .on_pan_end(move |e: &PanEvent| {
                            let f = df4.get();
                            store().crop_scale(f, e.translation.x.0 as f64);
                        })
                        .child(
                            view()
                                .h(px(3.0))
                                .rounded(px(1.5))
                                .bg(theme::HOVER),
                        )
                        .child(
                            view()
                                .absolute()
                                .left(move || {
                                    px(((store().draft_crop.get().s as f32 - 1.0) / 2.0) * (TRACK - 14.0))
                                })
                                .size(px(14.0))
                                .rounded(px(7.0))
                                .bg(theme::TEXT),
                        ),
                )
                .child(
                    view()
                        .w(px(14.0))
                        .items_center()
                        .label("Zoom in")
                        .child(text("+").text_size(px(15.0)).text_color(theme::DIM))
                        .on_press(|| store().crop_zoom(0.25)),
                ),
        )
        .child(
            view()
                .flex_row()
                .gap_2()
                .child(
                    view()
                        .flex_1()
                        .h(px(30.0))
                        .rounded(px(10.0))
                        .bg(theme::HOVER)
                        .items_center()
                        .justify_center()
                        .child(text("Restart").text_size(px(13.0)).text_color(theme::TEXT))
                        .on_press(|| {
                            let st = store();
                            st.draft_crop.set(Crop { s: 1.0, x: 0.0, y: 0.0 });
                            st.avatar_draft.set(None);
                        }),
                )
                .child(
                    view()
                        .flex_1()
                        .h(px(30.0))
                        .rounded(px(10.0))
                        .bg(theme::TEXT)
                        .items_center()
                        .justify_center()
                        .child(text("Set avatar").text_size(px(13.0)).text_color(theme::CARD2))
                        .on_press(|| store().set_avatar()),
                ),
        )
}

// ————————————————————————— the app rename modal —————————————————————————

/// `appRename`'s modal — a full-window scrim with a centered 320pt card:
/// field-specific title, the input, Cancel / Rename (App.tsx:4190).
pub(crate) fn rename_overlay() -> impl IntoElement {
    dynamic(|| {
        let st = store();
        let Some(r) = st.app_rename.get() else {
            return view().into_element();
        };
        let (title, placeholder) = match r.field.as_str() {
            "file" => ("Rename File Name".to_owned(), "file-name.tsx".to_owned()),
            "label" => (format!("Name Branch · v{}", r.v.unwrap_or(0)), "branch name (e.g. slow-ball)".to_owned()),
            _ => ("Rename Title".to_owned(), "App title".to_owned()),
        };
        view()
            .size_full()
            .absolute()
            .items_center()
            .justify_center()
            .bg(move || ac(if dark() { "#00000080" } else { "#00000040" }, theme::PANE.dark))
            .on_press(|| store().app_rename.set(None))
            .child(
                view()
                    .w(px(320.0))
                    .rounded(px(16.0))
                    .bg(theme::CARD2)
                    .border_1()
                    .border_color(theme::BORDER_F)
                    .p(px(16.0))
                    .gap(px(12.0))
                    .on_press(|| {})
                    .child(
                        text(title)
                            .text_size(px(15.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT),
                    )
                    .child(
                        view()
                            .rounded(px(10.0))
                            .border_1()
                            .border_color(theme::BORDER_S)
                            .bg(theme::MAIN)
                            .px(px(10.0))
                            .child(
                                input()
                                    .value(|| store().app_rename_text.get())
                                    .on_change(|v: &str| store().app_rename_text.set(v.to_owned()))
                                    .on_submit(|_: &str| store().save_app_rename())
                                    .placeholder(placeholder)
                                    .placeholder_color(|| color(theme::GHOST))
                                    .auto_focus(|| true)
                                    .text_size(px(14.0))
                                    .text_color(theme::TEXT)
                                    .py(px(9.0)),
                            ),
                    )
                    .child(
                        view()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(
                                view()
                                    .px(px(14.0))
                                    .py(px(8.0))
                                    .rounded(px(999.0))
                                    .child(
                                        text("Cancel")
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme::DIM),
                                    )
                                    .on_press(|| store().app_rename.set(None)),
                            )
                            .child(
                                view()
                                    .px(px(14.0))
                                    .py(px(8.0))
                                    .rounded(px(999.0))
                                    .bg(theme::TEXT)
                                    .child(
                                        text("Rename")
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme::CARD),
                                    )
                                    .on_press(|| store().save_app_rename()),
                            ),
                    ),
            )
            .into_element()
    })
}

/// A hex string → `AdaptiveColor` (both modes), `fallback` when it
/// doesn't parse.
fn ac(hex: &str, fallback: Hsla) -> AdaptiveColor {
    AdaptiveColor::from(Hsla::from_str(hex).unwrap_or(fallback))
}

/// `hue(name)` — the library tile's deterministic color (App.tsx:102).
fn hue(s: &str) -> AdaptiveColor {
    const AVATAR_HUES: [&str; 7] = [
        "#8e6ee8", "#4f9df7", "#37b4a6", "#e0703c", "#e2536e", "#5fa34a", "#b8894a",
    ];
    let i: usize = s.chars().map(|c| c as usize).sum::<usize>() % AVATAR_HUES.len();
    let c = Hsla::from_str(AVATAR_HUES[i]).unwrap_or(theme::ACCENT.dark);
    AdaptiveColor { light: c, dark: c }
}

/// `client::post` — the pane's own POST wrapper (the store's client is
/// `pub(crate)` already but the pane keeps its call sites short).
async fn client_post(route: &str, body: serde_json::Value) -> Option<serde_json::Value> {
    crate::client::post(route, body).await
}
