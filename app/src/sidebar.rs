//! The left column (`shell.tsx`'s sidebar): the app title, "New chat",
//! the thread list, and the agents catalog at the bottom.

use std::str::FromStr;

use nsh_core::ui::prelude::*;

use crate::avatar::bot_avatar;
use crate::icons::{icon, icon_sized};
use crate::live::store;
use crate::store::{RoxBot, ThreadMeta};
use crate::theme;

/// The rail/full sidebar — `sideEff` (`sideShown ? sideMode : "hidden"`).
pub(crate) fn sidebar() -> impl IntoElement {
    when(
        move || store().side_shown.get(),
        || {
            when(move || store().side_mode.get() == "full", full_sidebar)
                .otherwise(rail)
                .into_element()
        },
    )
}

/// `sideEff === "hidden"` (App.tsx:2454): the toggle floats right of the
/// traffic lights, on their line. Mounted after the main column (z 5) so
/// the top bar's drag strip never sits over it.
pub(crate) fn hidden_toggle_layer() -> impl IntoElement {
    when(
        move || !store().side_shown.get(),
        || {
            // `titleBar().height` can report a bogus toolbar measurement —
            // clamp to the 44pt bar we request (App.tsx:2456).
            #[cfg(target_os = "macos")]
            let bar = nsh_core::ui::desktop::title_bar();
            #[cfg(not(target_os = "macos"))]
            let bar = Edges::<Points>::default();
            let h = if bar.top.0 > 0.0 { bar.top.0.min(44.0) } else { 44.0 };
            let left = if bar.left.0 > 0.0 { bar.left.0 } else { 77.0 };
            view()
                .absolute()
                .top(px(h / 2.0 - 14.0))
                .left(px(left + 2.0))
                .z_index(5)
                .child(sidebar_toggle(28.0, 28.0, 7.0, Plate::None))
                .into_element()
        },
    )
}

/// The toggle's backing: Grok's `circleBtn` (a solid `#1f1f1f` disc), the
/// rail's shared hover plate, or nothing (the hidden-sidebar glyph).
#[derive(Clone, Copy, PartialEq)]
enum Plate {
    Solid,
    Hover,
    None,
}

/// `railPlate`'s hover fill — `tc("#f5f5f50f")` (App.tsx:1176).
fn plate_hover() -> AdaptiveColor {
    theme::tc("#f5f5f50f")
}

/// `panelGlyph` button (App.tsx:721): press toggles the sidebar
/// (`toggleSide`), right-click flips full↔rail (`toggleSideMode`) — the
/// same pair on every placement: full header, rail top, hidden.
fn sidebar_toggle(w: f32, h: f32, radius: f32, plate: Plate) -> impl IntoElement {
    let hovered = signal(false);
    view()
        .label("toggle sidebar")
        .child(
            view()
                .w(px(w))
                .h(px(h))
                .rounded(px(radius))
                .items_center()
                .justify_center()
                .bg(move || match plate {
                    Plate::Solid => theme::CARD,
                    Plate::Hover if hovered.get() => plate_hover(),
                    _ => AdaptiveColor::TRANSPARENT,
                })
                .child(icon("panel")),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(|| store().side_shown.update(|v| *v = !*v))
        .on_context_menu(|_: &ContextMenuEvent| {
            let st = store();
            st.side_shown.set(true);
            st.side_mode.set(
                if st.side_mode.get_untracked() == "full" {
                    "rail"
                } else {
                    "full"
                }
                .to_owned(),
            );
        })
}

/// `blurEdge` (App.tsx:1810) — a gradient strip melting the scroll's edge
/// into the sidebar color: `rgba(c,0.94) → 0.6@60% → 0` at the top,
/// mirrored at the bottom (`0 → 0.6@40% → 0.94`).
fn blur_edge(top: bool, h: f32) -> impl IntoElement {
    // Grok Bot's list mask (`linear-gradient(transparent 0, #000 28px, …
    // #000 calc(100% - 48px), transparent)`), drawn as the sidebar's color
    // fading to clear over the list's edge.
    let (a0, a2) = if top { (1.0, 0.0) } else { (0.0, 1.0) };
    view()
        .pointer_events(PointerEvents::None)
        .absolute()
        .left(px(0.0))
        .right(px(0.0))
        .h(px(h))
        .when(top, |v| v.top(px(0.0)))
        .when(!top, |v| v.bottom(px(0.0)))
        .z_index(2)
        .bg_linear_to_b()
        .gradient_from(theme::tone(
            theme::hsla(0.0, 0.0, 0.96, a0),
            theme::hsla(0.0, 0.0, 0.055, a0),
        ))
        .gradient_to(theme::tone(
            theme::hsla(0.0, 0.0, 0.96, a2),
            theme::hsla(0.0, 0.0, 0.055, a2),
        ))
}

/// `AVATAR_HUES` — the chat-initial circle colors (`hue(t.title)`).
const AVATAR_HUES: [&str; 7] = [
    "#1884fc", "#32bb78", "#f2994a", "#eb5757", "#9b51e0", "#2d9cdb", "#f2c94c",
];

/// `hue(s)` — the title's char-code sum picks a stable hue.
fn hue(s: &str) -> Hsla {
    let i: usize = s.chars().map(|c| c as usize).sum::<usize>() % AVATAR_HUES.len();
    Hsla::from_str(AVATAR_HUES[i]).unwrap_or(theme::MUTED.light)
}

/// The icon rail (`railView`, App.tsx:1830): bot plates for pinned + the
/// named groups, rules between, chat-initial tiles below.
fn rail() -> impl IntoElement {
    view()
        .w(px(88.0))
        .h_full()
        .pb_2()
        .bg(theme::SIDEBAR)
        .border_r(px(0.5))
        .border_r_color(theme::BORDER_F)
        .child(
            view()
                .flex_1()
                // `blurEdge("top", 44+54+6)` / `("bottom", 28)` melt the
                // scroll's ends into the panel (App.tsx:1894); the list
                // runs under the toggle row from the window's top.
                .child(blur_edge(true, 104.0))
                .child(blur_edge(false, 28.0))
                // the window-drag strip under the traffic lights, then the
                // toggle on a 54×44 hover plate (App.tsx:1896).
                .child(
                    view()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .h(px(44.0))
                        .z_index(3)
                        .window_drag(true),
                )
                .child(
                    view()
                        .absolute()
                        .top(px(44.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .h(px(54.0))
                        .items_center()
                        .justify_center()
                        .z_index(3)
                        .child(sidebar_toggle(54.0, 44.0, 10.0, Plate::Hover)),
                )
                .child(
                    scroll().flex_1().child(
                        view()
                            .items_center()
                            .gap_1()
                            .pt(px(44.0 + 54.0 + 4.0))
                            .pb(px(28.0))
                            .child(each(
                                || store().pinned_bots(),
                                |b| b.id.clone(),
                                |b, _| rail_btn(b),
                            ))
                    .child(each(
                        || store().bot_groups(),
                        |(k, _, _)| k.clone(),
                        |g, i| {
                            let g2 = g.clone();
                            let i2 = i.clone();
                            view()
                                .items_center()
                                .gap_1()
                                .child(when(
                                    move || {
                                        i2.get() > 0 || !store().pinned_bots().is_empty()
                                    },
                                    rail_rule,
                                ))
                                .child(each(
                                    move || g2.get().2,
                                    |b| b.id.clone(),
                                    |b, _| rail_btn(b),
                                ))
                        },
                    ))
                    .child(when(
                        || !store().chats().is_empty(),
                        || {
                            view()
                                .items_center()
                                .gap_1()
                                .child(rail_rule())
                                .child(each(
                                    || {
                                        store()
                                            .chats()
                                            .into_iter()
                                            .take(12)
                                            .collect::<Vec<_>>()
                                    },
                                    |t| t.id.clone(),
                                    |t, _| rail_chat(t),
                                ))
                        },
                    )),
                ),
            ),
        )
        // The rail footer — New / Plugins / Account on the shared 54×36
        // plates (App.tsx:1904; the account plate stays 54 tall).
        .child(
            view()
                .items_center()
                .gap(px(2.0))
                .pt(px(6.0))
                .pb_2()
                .child(rail_plate("newChat", "New Bot", 17.0, 36.0, || {
                    // `newMenu` opens below the button — the footer plate
                    // is near the window bottom, so anchor it a fixed
                    // distance up (the add-menu is ~5 rows).
                    crate::ctx::open_at(20.0, window().height.0 - 270.0, crate::ctx::add_items());
                }))
                .child(rail_plate("plugins", "Plugins", 13.0, 36.0, || {
                    store().open_plugins()
                }))
                .child(
                    view()
                        .size(px(54.0))
                        .rounded(px(10.0))
                        .items_center()
                        .justify_center()
                        .child(account_circle()),
                ),
        )
}

/// `railPlate(key, 36)` — the 54-wide hover plate with a sized icon,
/// shared by the rail's action buttons (App.tsx:1173).
fn rail_plate(
    name: &'static str,
    label: &'static str,
    icon_size: f32,
    h: f32,
    on: impl Fn() + 'static,
) -> impl IntoElement {
    let hovered = signal(false);
    view()
        .label(label)
        .child(
            view()
                .w(px(54.0))
                .h(px(h))
                .rounded(px(10.0))
                .items_center()
                .justify_center()
                .bg(move || if hovered.get() { plate_hover() } else { AdaptiveColor::TRANSPARENT })
                .child(icon_sized(name, icon_size)),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || on())
}

/// `newMenu` (App.tsx:973) — open the add-menu below the measured
/// button (or at the fallback corner when the measure is lost).
fn open_add_menu(node: ElementRef) {
    spawn(async move {
        let at = node.measure().await.map(|m| {
            (m.page.x.0, m.page.y.0 + m.frame.size.height.0.max(36.0) + 6.0)
        });
        let (x, y) = at.unwrap_or((20.0, 700.0));
        crate::ctx::open_at(x, y, crate::ctx::add_items());
    });
}

/// The header's "New" button (`circleBtn`, App.tsx:984): a solid
/// `tc("#1f1f1f")` plate that opens `addMenu` below itself on press.
fn new_menu_button(name: &'static str, label: &'static str, size: f32, radius: f32) -> impl IntoElement {
    let node = element_ref();
    view()
        .element_ref(&node)
        .label(label)
        .child(
            view()
                .size(px(size))
                .rounded(px(radius))
                .items_center()
                .justify_center()
                .bg(theme::CARD)
                .child(icon_sized(name, 18.0)),
        )
        .on_press(move || open_add_menu(node))
}

/// `railRule` — the 54×0.5 hairline between rail groups.
fn rail_rule() -> impl IntoElement {
    view()
        .w(px(54.0))
        .h(px(0.5))
        .bg(theme::BORDER_S)
        .my(px(6.0))
}

/// `railBtn` (App.tsx:1183) — the bot's rail plate: a 54px rounded plate
/// holding the 44px mark in a 36px cell, the `:rail` gaze, the run state
/// and the ambient overlay over a photo.
fn rail_btn(b: ReadSignal<RoxBot>) -> impl IntoElement {
    let b0 = b.get();
    let tid2 = b0.thread_id.clone();
    let bid = b0.id.clone();
    let bid3 = b0.id.clone();
    let name = b0.name.clone();
    let gaze = crate::avatar::BlobGaze::new();
    let gaze2 = gaze.clone();
    let gaze3 = gaze.clone();
    let hovered = signal(false);
    // `railPop` — measure the tile, then anchor the card at its edge
    // (the `popSeq` guard drops the answer if hover already ended).
    let node = element_ref();
    let state_src: std::rc::Rc<dyn Fn() -> String> =
        std::rc::Rc::new(move || store().bot_st(&bid));
    let state_src2 = state_src.clone();
    let state_src3 = state_src.clone();
    view()
        .element_ref(&node)
        .size(px(54.0))
        .rounded(px(10.0))
        .items_center()
        .justify_center()
        .bg(move || {
            if store().active_id.get() == tid2 {
                theme::tc("#f5f5f514")
            } else if hovered.get() {
                plate_hover()
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .child(
            view()
                .size(px(36.0))
                .items_center()
                .justify_center()
                .child(dynamic(move || {
                    let bv = b.get();
                    bot_avatar(crate::avatar::BotAvatarProps {
                        size: 44.0,
                        box_: Some(36.0),
                        state_src: Some(state_src.clone()),
                        emph_src: Some(std::rc::Rc::new({
                            let bid3 = bid3.clone();
                            move || store().emph_bot.get() == bid3
                        })),
                        gaze: Some(gaze2.clone()),
                        ..crate::avatar::BotAvatarProps::bot(&bv, 44.0)
                    })
                    .into_element()
                }))
                .child(when(
                    move || {
                        !b.get().image.is_empty()
                            && crate::statusfx::morph_kind(&state_src2()) != "none"
                    },
                    move || {
                        crate::statusfx::fx_overlay_fn(
                            state_src3.clone(),
                            36.0,
                            theme::FAINT.dark.to_hex(),
                        )
                    },
                )),
        )
        .on_hover_in(move || {
            hovered.set(true);
            let st = store();
            let seq = st.pop_seq.get() + 1;
            st.pop_seq.set(seq);
            let b_pop = b.get();
            spawn(async move {
                if let Some(m) = node.measure().await {
                    let st = store();
                    if st.pop_seq.get() == seq {
                        st.rail_pop.set(Some(crate::store::RailPop {
                            bot: b_pop,
                            cy: m.page.y.0 + m.frame.size.height.0 / 2.0,
                            x: m.page.x.0 + m.frame.size.width.0 + 6.0,
                        }));
                    }
                }
            });
        })
        .on_hover_move(move |e: &HoverEvent| {
            // `railGaze(b.id+":rail").follow(e, 27, 27, 18)` — plate center.
            gaze.follow(e.position.x.0 as f64, e.position.y.0 as f64, 27.0, 27.0, 18.0);
        })
        .on_hover_out(move || {
            hovered.set(false);
            let st = store();
            st.pop_seq.set(st.pop_seq.get() + 1);
            st.rail_pop.set(None);
            gaze3.rest();
        })
        .on_press(move || {
            let st = store();
            let bv = b.get();
            if !bv.thread_id.is_empty() {
                st.open_thread(bv.thread_id);
            } else {
                st.load_detail(bv.id);
                st.right_open.set(true);
            }
        })
        .on_context_menu(move |e: &ContextMenuEvent| {
            crate::ctx::open(e, crate::ctx::bot_items(&b.get()))
        })
        .label(name)
}

/// The rail's chat tile: a `hue(title)` circle with the title's first
/// letter (App.tsx:1877).
fn rail_chat(t: ReadSignal<ThreadMeta>) -> impl IntoElement {
    let t0 = t.get();
    let id = t0.id.clone();
    let id2 = t0.id.clone();
    let name = t0.name.clone();
    let hovered = signal(false);
    view()
        .size(px(54.0))
        .rounded(px(10.0))
        .items_center()
        .justify_center()
        .bg(move || {
            if store().active_id.get() == id2 {
                theme::tc("#f5f5f514")
            } else if hovered.get() {
                plate_hover()
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .child(
            view()
                .size(px(36.0))
                .rounded(px(18.0))
                .bg(move || hue(&t.get().name))
                .items_center()
                .justify_center()
                .child(
                    text(move || {
                        t.get()
                            .name
                            .chars()
                            .next()
                            .unwrap_or('?')
                            .to_uppercase()
                            .collect::<String>()
                    })
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(white()),
                ),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || store().open_thread(id.clone()))
        .on_context_menu(move |e: &ContextMenuEvent| {
            crate::ctx::open(e, crate::ctx::chat_items(&t.get()))
        })
        .label(name)
}

fn full_sidebar() -> impl IntoElement {
    view()
        .w(px(240.0))
        .h_full()
        .bg(theme::SIDEBAR)
        .border_r(px(0.5))
        .border_r_color(theme::BORDER_F)
        .child(
            // Header: traffic lights left, round controls right — toggle +
            // New — on the sidebar's own solid strip, as Grok Bot's: the
            // list starts below it and never shows behind the lights.
            view()
                .h(px(60.0))
                .flex_row()
                .items_center()
                .justify_end()
                .gap_2()
                .pr(px(16.0))
                .window_drag(true)
                .child(sidebar_toggle(36.0, 36.0, 18.0, Plate::Solid))
                .child(new_menu_button("newChat", "New Bot", 36.0, 18.0)),
        )
        .child(
            view()
                .flex_1()
                // The list's edges fade (Grok Bot's 28 / 48pt mask).
                .child(blur_edge(true, 28.0))
                .child(blur_edge(false, 48.0))
                .child(
                    scroll().flex_1().child(
                        view()
                            .pt(px(4.0))
                            .px(px(12.0))
                            .pb(px(32.0))
                            .gap(px(10.0))
                            // `pinnedBots` — the wrap row of tiles (App.tsx:1967).
                    .child(when(
                        || !store().pinned_bots().is_empty(),
                        || {
                            view()
                                .flex_row()
                                .flex_wrap()
                                .justify_center()
                                .gap_2()
                                .pt(px(6.0))
                                .pb_2()
                                .child(each(
                                    || store().pinned_bots(),
                                    |b| b.id.clone(),
                                    |b, _| pin_tile(b),
                                ))
                        },
                    ))
                    // `botGroups` — named sections + "Unassigned", each a
                    // collapsible `secHeader` block.
                    .child(each(
                        || store().bot_groups(),
                        |(k, _, _)| k.clone(),
                        |g, _| {
                            let g2 = g.clone();
                            let (key, title) = {
                                let v = g.get();
                                (v.0.clone(), v.1.clone())
                            };
                            let key2 = key.clone();
                            view()
                                .child(sec_header(key, title, None))
                                .child(when(
                                    move || !collapsed(&key2),
                                    move || {
                                        let g3 = g2.clone();
                                        each(
                                            move || g3.get().2,
                                            |b| b.id.clone(),
                                            |b, _| bot_row(b),
                                        )
                                    },
                                ))
                        },
                    ))
                    // Mini Apps — apps pinned from their cards (⋯ → Pin to
                    // Sidebar); only while there are any.
                    .child(when(
                        || !store().pinned_apps.get().is_empty(),
                        || {
                            view()
                                .child(sec_header("__apps".to_owned(), "Mini Apps".to_owned(), None))
                                .child(when(
                                    || !collapsed("__apps"),
                                    || {
                                        each(
                                            || store().pinned_apps.get(),
                                            |p| p.app.clone(),
                                            |p, _| pinned_app_row(p),
                                        )
                                    },
                                ))
                        },
                    ))
                    // `chats` — the plain list; bot threads live behind
                    // their persona rows.
                    .child(sec_header("__chats".to_owned(), "Chats".to_owned(), None))
                    .child(when(
                        || !collapsed("__chats"),
                        || {
                            each(
                                || store().chats(),
                                |t| t.id.clone(),
                                |t, _| thread_row(t),
                            )
                        },
                    ))
                    // `hiddenBots` / `hiddenChats` / `archivedChats` —
                    // collapsed by default (`collapsedSecs` init).
                    .child(when(
                        || !store().hidden_bots().is_empty(),
                        || {
                            view()
                                .child(sec_header(
                                    "__hidden".to_owned(),
                                    "Hidden Bots".to_owned(),
                                    Some(std::rc::Rc::new(|| store().hidden_bots().len())),
                                ))
                                .child(when(
                                    || !collapsed("__hidden"),
                                    || {
                                        each(
                                            || store().hidden_bots(),
                                            |b| b.id.clone(),
                                            |b, _| bot_row(b),
                                        )
                                    },
                                ))
                        },
                    ))
                    .child(when(
                        || !store().hidden_chats().is_empty(),
                        || {
                            view()
                                .child(sec_header(
                                    "__hiddenChats".to_owned(),
                                    "Hidden Chats".to_owned(),
                                    Some(std::rc::Rc::new(|| store().hidden_chats().len())),
                                ))
                                .child(when(
                                    || !collapsed("__hiddenChats"),
                                    || {
                                        each(
                                            || store().hidden_chats(),
                                            |t| t.id.clone(),
                                            |t, _| thread_row(t),
                                        )
                                    },
                                ))
                        },
                    ))
                    .child(when(
                        || !store().archived_chats().is_empty(),
                        || {
                            view()
                                .child(sec_header(
                                    "__archived".to_owned(),
                                    "Archived".to_owned(),
                                    Some(std::rc::Rc::new(|| store().archived_chats().len())),
                                ))
                                .child(when(
                                    || !collapsed("__archived"),
                                    || {
                                        each(
                                            || store().archived_chats(),
                                            |t| t.id.clone(),
                                            |t, _| thread_row(t),
                                        )
                                    },
                                ))
                        },
                    )),
                ),
            ),
        )
        // The footer — account circle (→ popover) + the Plugins pill
        // (App.tsx:2024).
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_4()
                .pb_4()
                .child(account_circle())
                .child(
                    view()
                        .flex_1()
                        .h(px(36.0))
                        .rounded(px(18.0))
                        .bg(theme::CARD)
                        .flex_row()
                        .items_center()
                        .gap(px(7.0))
                        .px(px(14.0))
                        .child(icon_sized("plugins", 11.0))
                        .child(
                            text("Plugins")
                                .text_size(px(14.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme::TEXT)
                                .number_of_lines(1),
                        )
                        .on_press(|| store().open_plugins())
                        .label("Plugins"),
                ),
        )
}

/// `accountOpen` circle — Grok's "UI" initials tile (circleBtn +
/// `tc("#1f1f1f")` = `CARD`); the rail's plate is the same tile (App.tsx
/// 1910 & 2028).
fn account_circle() -> impl IntoElement {
    view()
        .size(px(36.0))
        .rounded(px(18.0))
        .bg(move || {
            if store().account_open.get() {
                theme::CARD2
            } else {
                theme::CARD
            }
        })
        .items_center()
        .justify_center()
        .child(
            text("UI")
                .text_size(px(14.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::DIM),
        )
        .on_press(|| store().account_open.update(|v| *v = !*v))
        .label("Account")
}

/// The account popover (App.tsx:3099): scrim closes on press +
/// right-click; the card anchors above the footer circle — header (avatar
/// + "uriah / Local account"), Mini Apps / Plugins / Settings, then the
/// inert account rows.
pub(crate) fn account_pop_layer() -> impl IntoElement {
    dynamic(|| {
        if !store().account_open.get() {
            return view().into_element();
        }
        fn pop_row(label: &'static str, icon_name: &'static str, on: impl Fn() + 'static) -> View {
            let hovered = signal(false);
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .h(px(30.0))
                .rounded(px(6.0))
                .bg(move || {
                    if hovered.get() {
                        theme::HOVER
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .child(view().w(px(16.0)).items_center().child(icon(icon_name)))
                .child(text(label).text_size(px(13.0)).text_color(theme::TEXT))
                .on_hover_in(move || hovered.set(true))
                .on_hover_out(move || hovered.set(false))
                .on_press(move || on())
        }
        let divider = || {
            view()
                .h(px(0.5))
                .mx_1()
                .my_1()
                .bg(theme::BORDER_S)
        };
        view()
            .absolute()
            .size_full()
            .child(
                view()
                    .absolute()
                    .size_full()
                    .on_press(|| store().account_open.set(false))
                    .on_context_menu(|_: &ContextMenuEvent| store().account_open.set(false)),
            )
            .child(
                view()
                    .absolute()
                    .left(px(12.0))
                    .bottom(px(64.0))
                    .w(px(240.0))
                    .bg(theme::MENU_BG)
                    .rounded(px(12.0))
                    .border_1()
                    .border_color(theme::BORDER_S)
                    .p(px(5.0))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(10.0))
                            .px(px(10.0))
                            .py(px(10.0))
                            .child(
                                view()
                                    .size(px(32.0))
                                    .rounded(px(16.0))
                                    .bg(theme::CARD)
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        text("UI")
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme::TEXT),
                                    ),
                            )
                            .child(
                                view()
                                    .flex_1()
                                    .child(
                                        text("uriah")
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme::TEXT),
                                    )
                                    .child(
                                        text("Local account")
                                            .text_size(px(12.0))
                                            .text_color(theme::DIM),
                                    ),
                            ),
                    )
                    .child(divider())
                    .child(pop_row("Plugins", "plugins", || {
                        store().open_plugins();
                        store().account_open.set(false);
                    }))
                    .child(pop_row("Settings", "settings", || {
                        store().account_open.set(false);
                    }))
                    .child(divider())
                    .child(pop_row("Add account", "addAccount", || {
                        store().account_open.set(false);
                    }))
                    .child(pop_row("Log out", "logout", || {
                        store().account_open.set(false);
                    })),
            )
            .into_element()
    })
}

/// `collapsedSecs()[key]` — absent means open (named sections); the
/// `__hidden*`/`__archived` keys start collapsed from the store's init.
fn collapsed(key: &str) -> bool {
    store()
        .collapsed
        .get()
        .get(key)
        .copied()
        .unwrap_or(false)
}

/// `secHeader` — a tappable section title; the chevron appears on hover
/// and the count sits beside it (`toggleSec` on press).
fn sec_header(
    key: String,
    title: String,
    count: Option<std::rc::Rc<dyn Fn() -> usize>>,
) -> impl IntoElement {
    let key2 = key.clone();
    let key3 = key.clone();
    let hovered = signal(false);
    view()
        .h(px(30.0))
        .flex_row()
        .items_center()
        .pt(px(8.0))
        .pb(px(6.0))
        .px_2()
        .rounded(px(6.0))
        .gap(px(6.0))
        .bg(move || {
            if hovered.get() {
                theme::HOVER
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .child(
            text(title)
                .text_size(px(12.0))
                .text_color(theme::MUTED)
                .flex_1()
                .number_of_lines(1),
        )
        .when(count.is_some(), |v| {
            v.child(
                text(move || count.as_ref().map(|c| c().to_string()).unwrap_or_default())
                    .text_size(px(12.0))
                    .text_color(theme::MUTED),
            )
        })
        .child(
            text(move || {
                if store()
                    .collapsed
                    .get()
                    .get(&key3)
                    .copied()
                    .unwrap_or(false)
                {
                    "›"
                } else {
                    "⌄"
                }
            })
            .text_size(px(13.0))
            .text_color(theme::MUTED)
            .opacity(move || if hovered.get() { 1.0f32 } else { 0.0f32 }),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || {
            // `toggleSec(k)`
            let key = key2.clone();
            store().collapsed.update(|m| {
                let e = m.entry(key).or_insert(false);
                *e = !*e;
            });
        })
}

/// A Mini Apps row: the app's `</>` tile on its hue and its title; a
/// press opens it in its own window, the context menu shows the card in
/// its chat or unpins it.
fn pinned_app_row(p: ReadSignal<crate::store::PinnedApp>) -> impl IntoElement {
    let p0 = p.get_untracked();
    let (app, app_ctx) = (p0.app.clone(), p0.app.clone());
    let hovered = signal(false);
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .py(px(7.0))
        .rounded(px(8.0))
        .bg(move || if hovered.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || {
            let st = store();
            if let Some(p) = st.pinned(&app) {
                st.popout_sized(&p.app, p.w, p.h, false);
            }
        })
        .on_context_menu(move |e: &ContextMenuEvent| {
            crate::ctx::open(e, crate::ctx::pinned_items(&app_ctx))
        })
        .child(
            view()
                .size(px(36.0))
                .rounded(px(9.0))
                .bg(hue(crate::store::app_name(&p0.app)))
                .items_center()
                .justify_center()
                .child(
                    text("</>")
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(hsla(0.0, 0.0, 1.0, 1.0)),
                ),
        )
        .child(
            view()
                .flex_1()
                .child(
                    text(move || p.get().title)
                        .text_size(px(13.0))
                        .text_color(theme::TEXT)
                        .number_of_lines(1),
                )
                .child(text("Mini app").text_size(px(12.0)).text_color(theme::MUTED)),
        )
}

fn thread_row(t: ReadSignal<ThreadMeta>) -> impl IntoElement {
    let st = store();
    let id = t.get().id.clone();
    let tid = t.get().id.clone();
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
                    // `chatRow`'s 36px letter circle (App.tsx:1091).
                    view()
                        .size(px(36.0))
                        .rounded(px(18.0))
                        .bg(move || hue(&t.get().name))
                        .items_center()
                        .justify_center()
                        .child(
                            text(move || {
                                t.get()
                                    .name
                                    .chars()
                                    .next()
                                    .unwrap_or('?')
                                    .to_uppercase()
                                    .collect::<String>()
                            })
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(0.0, 0.0, 1.0, 1.0)),
                        ),
                )
                .child(
                    view()
                        .flex_1()
                        .child(
                            text(move || {
                                let v = t.get();
                                if v.name.is_empty() { "New chat".to_owned() } else { v.name }
                            })
                            .text_size(px(13.0))
                            .text_color(theme::TEXT)
                            .number_of_lines(1),
                        )
                        .child(
                            text(move || format!("{} messages", t.get().count))
                                .text_size(px(11.0))
                                .text_color(theme::GHOST)
                                .number_of_lines(1),
                        ),
                ),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || st.open_thread(id.clone()))
        .on_context_menu(move |e: &ContextMenuEvent| {
            crate::ctx::open(e, crate::ctx::chat_items(&t.get()))
        })
}

fn bot_row(b: ReadSignal<RoxBot>) -> impl IntoElement {
    let hovered = signal(false);
    let b0 = b.get();
    let bid = b0.id.clone();
    let bid3 = b0.id.clone();
    // `railGaze(b.id + ":row")` — the mark follows the pointer over the row.
    let gaze = crate::avatar::BlobGaze::new();
    let gaze2 = gaze.clone();
    let gaze3 = gaze.clone();
    let state_src: std::rc::Rc<dyn Fn() -> String> =
        std::rc::Rc::new(move || store().bot_st(&bid));
    let state_src2 = state_src.clone();
    let state_src3 = state_src.clone();
    view()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py(px(7.0))
                .rounded(px(8.0))
                // The open bot's row is lit, as a chat row is (Grok's
                // active row).
                .bg(move || {
                    let open = {
                        let tid = b.get().thread_id;
                        !tid.is_empty() && store().active_id.get() == tid
                    };
                    if open {
                        theme::ACTIVE
                    } else if hovered.get() {
                        theme::HOVER
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .child(
                    // Grok's 36px cell holding the 44px BotAvatar
                    // (box = 44*0.82 = 36).
                    view()
                        .size(px(36.0))
                        .items_center()
                        .justify_center()
                        .child(dynamic(move || {
                            let bv = b.get();
                            bot_avatar(crate::avatar::BotAvatarProps {
                                size: 44.0,
                                box_: Some(36.0),
                                state_src: Some(state_src.clone()),
                                // `emphasized={emphBot() === b().id}` — the
                                // status row's hover reaches this mark too.
                                emph_src: Some(std::rc::Rc::new({
                                    let bid3 = bid3.clone();
                                    move || store().emph_bot.get() == bid3
                                })),
                                gaze: Some(gaze2.clone()),
                                ..crate::avatar::BotAvatarProps::bot(&bv, 44.0)
                            })
                            .into_element()
                        }))
                        .child(main_badge(b, 15.0))
                        // `FxOverlay` over a photo avatar while the run's
                        // morph is ambient-backed.
                        .child(when(
                            move || {
                                !b.get().image.is_empty()
                                    && crate::statusfx::morph_kind(&state_src2()) != "none"
                            },
                            move || {
                                crate::statusfx::fx_overlay_fn(
                                    state_src3.clone(),
                                    36.0,
                                    theme::FAINT.dark.to_hex(),
                                )
                            },
                        )),
                )
                .child(
                    view()
                        .flex_1()
                        .child(
                            text(move || b.get().name)
                                .text_size(px(13.0))
                                .text_color(theme::TEXT)
                                .number_of_lines(1),
                        )
                        .child(
                            text(move || b.get().desc)
                                .text_size(px(11.0))
                                .text_color(theme::GHOST)
                                .number_of_lines(1),
                        ),
                ),
        )
        .on_hover_in(move || hovered.set(true))
        .on_hover_move(move |e: &HoverEvent| {
            // `railGaze(...).follow(e, 26, 27, 18)` — the 36px cell's center
            // in row coordinates: here 12px pad + 18px half-cell = 30, and
            // the row half-height is (7 + 36 + 7) / 2 = 25.
            gaze.follow(e.position.x.0 as f64, e.position.y.0 as f64, 30.0, 25.0, 18.0);
        })
        .on_hover_out(move || {
            hovered.set(false);
            gaze3.rest();
        })
        .on_press(move || {
            let st = store();
            // `selectThread(b.threadId)` — open the persona's chat; the
            // detail pane stays the fallback for a bot with none. Read the
            // row's signal: a `threadId` captured at mount goes stale when
            // `/threads` refreshes.
            let bv = b.get();
            if !bv.thread_id.is_empty() {
                st.open_thread(bv.thread_id);
            } else {
                st.load_detail(bv.id);
                st.right_open.set(true);
            }
        })
        .on_context_menu(move |e: &ContextMenuEvent| {
            crate::ctx::open(e, crate::ctx::bot_items(&b.get()))
        })
}

/// `pinTile` (App.tsx:1108) — a pinned bot's 95×92 tile: the 72px mark in
/// a 60px cell (with the `:pin` gaze), the ambient overlay over a photo,
/// and the name under it.
/// The primary Bot's star (Grok Bot's "Main Bot" badge): an orange disc
/// ringed in the page color on the avatar's bottom-right.
fn main_badge(b: ReadSignal<RoxBot>, size: f32) -> impl IntoElement {
    when(
        move || store().is_main_bot(&b.get().id),
        move || {
            view()
                .absolute()
                .right(px(-2.0))
                .bottom(px(-2.0))
                .size(px(size))
                .rounded(px(size / 2.0))
                .items_center()
                .justify_center()
                .bg(theme::tone(theme::hsla(0.098, 0.92, 0.52, 1.0), theme::hsla(0.098, 0.92, 0.52, 1.0)))
                .border(px(1.5))
                .border_color(theme::SIDEBAR)
                .label("Main Bot")
                .child(
                    text("★")
                        .text_size(px(size * 0.6))
                        .text_color(hsla(0.0, 0.0, 1.0, 1.0)),
                )
                .into_element()
        },
    )
}

fn pin_tile(b: ReadSignal<RoxBot>) -> impl IntoElement {
    let b0 = b.get();
    let name = b0.name.clone();
    let tid2 = b0.thread_id.clone();
    let bid = b0.id.clone();
    let bid3 = b0.id.clone();
    let gaze = crate::avatar::BlobGaze::new();
    let gaze2 = gaze.clone();
    let gaze3 = gaze.clone();
    // `railPop` — same hover card as the rail tiles.
    let node = element_ref();
    let state_src: std::rc::Rc<dyn Fn() -> String> =
        std::rc::Rc::new(move || store().bot_st(&bid));
    let state_src2 = state_src.clone();
    let state_src3 = state_src.clone();
    view()
        .element_ref(&node)
        .w(px(95.0))
        .h(px(92.0))
        .rounded(px(10.0))
        .pt(px(6.0))
        .px_2()
        .pb(px(4.0))
        .gap(px(6.0))
        .items_center()
        .bg(move || {
            if store().active_id.get() == tid2 {
                theme::ACTIVE
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .child(
            view()
                .size(px(60.0))
                .items_center()
                .justify_center()
                .child(dynamic(move || {
                    let bv = b.get();
                    bot_avatar(crate::avatar::BotAvatarProps {
                        size: 72.0,
                        box_: Some(60.0),
                        state_src: Some(state_src.clone()),
                        emph_src: Some(std::rc::Rc::new({
                            let bid3 = bid3.clone();
                            move || store().emph_bot.get() == bid3
                        })),
                        gaze: Some(gaze2.clone()),
                        ..crate::avatar::BotAvatarProps::bot(&bv, 72.0)
                    })
                    .into_element()
                }))
                .child(main_badge(b, 20.0))
                .child(when(
                    move || {
                        !b.get().image.is_empty()
                            && crate::statusfx::morph_kind(&state_src2()) != "none"
                    },
                    move || {
                        crate::statusfx::fx_overlay_fn(
                            state_src3.clone(),
                            60.0,
                            theme::FAINT.dark.to_hex(),
                        )
                    },
                )),
        )
        .child(
            text(move || b.get().name)
                .text_size(px(12.0))
                .text_color(theme::TEXT)
                .number_of_lines(1)
                .max_w(px(79.0)),
        )
        // Its label under the name (Grok Bot's tile).
        .child(when(
            move || !b.get().label.is_empty(),
            move || {
                text(move || b.get().label)
                    .text_size(px(11.0))
                    .text_color(theme::MUTED)
                    .number_of_lines(1)
                    .max_w(px(79.0))
                    .into_element()
            },
        ))
        .on_hover_in(move || {
            let st = store();
            let seq = st.pop_seq.get() + 1;
            st.pop_seq.set(seq);
            let b_pop = b.get();
            spawn(async move {
                if let Some(m) = node.measure().await {
                    let st = store();
                    if st.pop_seq.get() == seq {
                        st.rail_pop.set(Some(crate::store::RailPop {
                            bot: b_pop,
                            cy: m.page.y.0 + m.frame.size.height.0 / 2.0,
                            x: m.page.x.0 + m.frame.size.width.0 + 6.0,
                        }));
                    }
                }
            });
        })
        .on_hover_move(move |e: &HoverEvent| {
            // `railGaze(b.id+":pin").follow(e, 47.5, 36, 30)` — the tile's
            // center (95/2=47.5) and mark row's center (~36px from top).
            gaze.follow(e.position.x.0 as f64, e.position.y.0 as f64, 47.5, 36.0, 30.0);
        })
        .on_hover_out(move || {
            let st = store();
            st.pop_seq.set(st.pop_seq.get() + 1);
            st.rail_pop.set(None);
            gaze3.rest();
        })
        .on_press(move || {
            let st = store();
            let bv = b.get();
            if !bv.thread_id.is_empty() {
                st.open_thread(bv.thread_id);
            } else {
                st.load_detail(bv.id);
                st.right_open.set(true);
            }
        })
        .on_context_menu(move |e: &ContextMenuEvent| {
            crate::ctx::open(e, crate::ctx::bot_items(&b.get()))
        })
        .label(name)
}

/// `railPop` (App.tsx:3011) — the bot hover card: avatar + name over a
/// 2-line preview, vertically centered on the hovered element's measured
/// center at its right edge + 6px.
pub(crate) fn rail_pop_layer() -> impl IntoElement {
    dynamic(|| {
        let Some(pop) = store().rail_pop.get() else {
            return view().into_element();
        };
        let preview = if pop.bot.desc.is_empty() {
            "No messages yet".to_owned()
        } else {
            pop.bot.desc.clone()
        };
        // h = pad(20) + name row(18) + gap(4) + lines(≤2 × 18)
        let lines = ((preview.len() as f32 / 34.0).ceil().max(1.0)).min(2.0);
        let h = 20.0 + 18.0 + 4.0 + lines * 18.0;
        let top = (pop.cy - h / 2.0).max(8.0);
        view()
            .absolute()
            .left(px(pop.x))
            .top(px(top))
            .w(px(260.0))
            .p(px(10.0))
            .gap(px(4.0))
            .rounded(px(12.0))
            .bg(theme::MENU_BG)
            .border_1()
            .border_color(theme::BORDER_S)
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .h(px(18.0))
                    .child(
                        view()
                            .size(px(16.0))
                            .items_center()
                            .justify_center()
                            .child(bot_avatar(crate::avatar::BotAvatarProps {
                                size: 20.0,
                                box_: Some(16.0),
                                paused: true,
                                ..crate::avatar::BotAvatarProps::bot(&pop.bot, 20.0)
                            })),
                    )
                    .child(
                        text(pop.bot.name.clone())
                            .flex_1()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::TEXT)
                            .number_of_lines(1),
                    ),
            )
            .child(
                text(preview)
                    .text_size(px(13.0))
                    .text_color(theme::MUTED)
                    .number_of_lines(2),
            )
            .into_element()
    })
}
