//! The left column (`shell.tsx`'s sidebar): the app title, "New chat",
//! the thread list, and the agents catalog at the bottom.

use std::str::FromStr;

use nsh_core::prelude::*;

use crate::avatar::bot_avatar;
use crate::icons::{icon, icon_sized};
use crate::live::store;
use crate::store::{GroupRow, RoxApp, RoxBot, RoxGroup, ThreadMeta};
use crate::theme;

/// The rail/full sidebar — `sideEff` (`sideShown ? sideMode : "hidden"`).
pub(crate) fn sidebar() -> impl IntoElement {
    let shows = |full: bool| {
        move || {
            let st = store();
            st.side_shown.get() && (st.side_mode.get() == "full") == full
        }
    };
    view()
        .flex_row()
        .h_full()
        .child(kept(shows(true), 240.0, || full_sidebar().into_element()))
        .child(kept(shows(false), 88.0, || rail().into_element()))
}

/// A form of the sidebar `w` wide, made the first time it shows and kept:
/// hidden, it takes no width, draws nothing and takes no input. Made anew
/// on every ⌘B / ⌘⇧H it mounted ~300 views, a 15–20 ms frame. Not
/// clipped while it shows: the rail's tooltips reach over the chat.
fn kept(
    shown: impl Fn() -> bool + Copy + 'static,
    w: f32,
    make: impl Fn() -> AnyElement + 'static,
) -> impl IntoElement {
    let seen = signal(false);
    effect(move || {
        if shown() && !seen.get_untracked() {
            seen.set(true);
        }
    });
    view()
        .h_full()
        .w(move || px(if shown() { w } else { 0.0 }))
        .opacity(move || if shown() { 1.0f32 } else { 0.0 })
        .pointer_events(move || if shown() { PointerEvents::Auto } else { PointerEvents::None })
        .child(when(move || seen.get(), make))
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
            let bar = nsh_core::element::desktop::title_bar();
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
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
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
fn blur_edge(top: bool, h: f32, on: Signal<bool>) -> impl IntoElement {
    // Grok Bot's list mask (`linear-gradient(transparent 0, #000 28px, …
    // #000 calc(100% - 48px), transparent)`), drawn as the sidebar's color
    // fading to clear over the list's edge.
    let (a0, a2) = if top { (1.0, 0.0) } else { (0.0, 1.0) };
    view()
        .pointer_events(PointerEvents::None)
        .absolute()
        .left(px(0.0))
        .right(px(0.0))
        // Only while there is content past this edge (Grok Bot's fades
        // switch off at the list's ends).
        .opacity(move || if on.get() { 1.0f32 } else { 0.0 })
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
/// The rail's solid top: the drag strip (30pt) and the toggle row.
const RAIL_TOP: f32 = 30.0 + 36.0 + 4.0;

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
                // The list stops at the footer (nothing draws under it).
                .overflow_hidden()
                // The top strip is solid sidebar: the window-drag area under
                // the traffic lights, then the toggle right below them (a
                // 44×36 hover plate). The list scrolls under it and melts
                // into it below the toggle; the bottom edge melts into the
                // footer.
                .child(
                    view()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .h(px(RAIL_TOP))
                        .z_index(3)
                        .bg(theme::SIDEBAR)
                        .child(view().absolute().top(px(0.0)).left(px(0.0)).right(px(0.0)).h(px(30.0)).window_drag(true))
                        .child(
                            view()
                                .absolute()
                                .top(px(30.0))
                                .left(px(0.0))
                                .right(px(0.0))
                                .items_center()
                                .child(sidebar_toggle(44.0, 36.0, 9.0, Plate::Hover)),
                        ),
                )
                .child(view().absolute().top(px(RAIL_TOP)).left(px(0.0)).right(px(0.0)).child(blur_edge(true, 28.0, signal(true))))
                .child(blur_edge(false, 28.0, signal(true)))
                .child(
                    scroll().flex_1().child(
                        view()
                            .items_center()
                            .gap_1()
                            .pt(px(RAIL_TOP + 8.0))
                            .pb(px(28.0))
                            .child(each(
                                || store().pinned_bots(),
                                |b| b.id.clone(),
                                |b, _| rail_btn(b),
                            ))
                    // A group collapsed in the full sidebar is left out here
                    // (Grok Bot's rail), as are empty ones and collapsed
                    // Unassigned / Chats. Rebuilt whole when the groups or
                    // their members change (a few rows): keyed `each`s
                    // nested here dropped a whole open group when its
                    // members' records changed ("a kept row is gone") —
                    // after any reply that reloads the groups.
                    .child(dynamic(|| {
                        let st = store();
                        let pinned = !st.pinned_bots().is_empty();
                        let open: Vec<(String, Vec<GroupRow>)> = st
                            .groups
                            .get()
                            .into_iter()
                            .filter(|g| !collapsed(&g.id))
                            .map(|g| (g.id.clone(), st.group_rows(&g.id)))
                            .filter(|(_, rows)| !rows.is_empty())
                            .collect();
                        view()
                            .items_center()
                            .gap_1()
                            .children(open.into_iter().enumerate().map(|(i, (gid, rows))| {
                                view()
                                    .items_center()
                                    .gap_1()
                                    .when(i > 0 || pinned, |v| v.child(rail_rule()))
                                    .children(rows.into_iter().map(|m| rail_member(gid.clone(), m)))
                                    .into_element()
                            }))
                            .into_element()
                    }))
                    .child(when(
                        || !store().ungrouped_bots().is_empty() && !collapsed("__un"),
                        || {
                            view()
                                .items_center()
                                .gap_1()
                                .child(when(
                                    || {
                                        !store().pinned_bots().is_empty()
                                            || store().groups.get().iter().any(|g| {
                                                !collapsed(&g.id) && !store().group_rows(&g.id).is_empty()
                                            })
                                    },
                                    rail_rule,
                                ))
                                .child(each(
                                    || store().ungrouped_bots(),
                                    |b| b.id.clone(),
                                    |b, _| rail_btn(b),
                                ))
                                .into_element()
                        },
                    ))
                    .child(when(
                        || !store().chats().is_empty() && !collapsed("__chats"),
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
        // The rail footer, as Grok Bot's: New Bot (＋) and Connect apps
        // (grid) as 28pt circles with its own 16pt glyphs at 64% ink, 6pt
        // apart, then the 54pt account plate; the list ends where it
        // starts.
        .child(
            view()
                .items_center()
                .gap(px(6.0))
                // Its tooltips reach past the rail, over the chat.
                .z_index(30)
                .child(rail_circle("grokPlus", "New Bot", || {
                    crate::ctx::open_at(20.0, window().height.0 - 270.0, crate::ctx::add_items());
                }))
                .child(rail_circle("grokGridPlus", "Connect apps", || store().open_plugins()))
                .child(rail_account()),
        )
}

/// Grok Bot's rail hover fill (`rgba(245,245,245,.04)` on dark).
fn rail_hover() -> AdaptiveColor {
    theme::tone(theme::hsla(0.0, 0.0, 0.11, 0.04), theme::hsla(0.0, 0.0, 0.96, 0.04))
}

/// A rail footer button, Grok Bot's: a 28pt circle with its 16pt glyph at
/// 64% ink — 90% and a 4% fill while hovered — and, after a moment, its
/// label in a tooltip 8pt above, left-aligned 8pt from the rail's edge.
fn rail_circle(name: &'static str, label: &'static str, on: impl Fn() + 'static) -> impl IntoElement {
    let hovered = signal(false);
    let tip = signal(false);
    let epoch = std::rc::Rc::new(std::cell::Cell::new(0u64));
    let epoch2 = epoch.clone();
    view()
        .label(label)
        .size(px(28.0))
        .rounded(px(14.0))
        .items_center()
        .justify_center()
        .bg(move || if hovered.get() { rail_hover() } else { AdaptiveColor::TRANSPARENT })
        .child(crate::icons::icon_ink(name, 16.0, move || {
            let a = if hovered.get() { 0.9 } else { 0.64 };
            theme::tone(theme::hsla(0.0, 0.0, 0.11, a), theme::hsla(0.0, 0.0, 0.96, a))
        }))
        .child(when(
            move || tip.get(),
            move || {
                view()
                    .absolute()
                    .bottom(px(28.0 + 8.0))
                    // The rail's 8pt edge: the button sits 30pt in.
                    .left(px(-22.0))
                    .z_index(40)
                    .pointer_events(PointerEvents::None)
                    // Its own width: the label on one line plus the padding
                    // (inside the 28pt button it was squeezed to "Con…").
                    .w(px(measure_text("420 12px -apple-system, system-ui, sans-serif", label).0.ceil() + 12.0))
                    .py(px(4.0))
                    .px(px(6.0))
                    .rounded(px(6.0))
                    .bg(theme::tone(theme::hsla(0.0, 0.0, 1.0, 1.0), theme::hsla(0.0, 0.0, 0.122, 1.0)))
                    .child(
                        text(label)
                            .text_size(px(12.0))
                            .line_height(px(16.0))
                            .font_weight(FontWeight(420))
                            .text_color(theme::TEXT)
                            .number_of_lines(1),
                    )
                    .into_element()
            },
        ))
        .on_mouse_enter(move |_: &MouseEvent| {
            hovered.set(true);
            let e = epoch.get().wrapping_add(1);
            epoch.set(e);
            let epoch = epoch.clone();
            spawn(async move {
                timer(std::time::Duration::from_millis(500)).await;
                if epoch.get() == e {
                    tip.set(true);
                }
            });
        })
        .on_mouse_leave(move |_: &MouseEvent| {
            epoch2.set(epoch2.get().wrapping_add(1));
            hovered.set(false);
            tip.set(false);
        })
        .on_press(move || {
            tip.set(false);
            on()
        })
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
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
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
        .on_mouse_enter(move |_: &MouseEvent| {
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
                            app: String::new(),
                        }));
                    }
                }
            });
        })
        .on_mouse_move(move |e: &MouseEvent| {
            // `railGaze(b.id+":rail").follow(e, 27, 27, 18)` — plate center.
            gaze.follow(e.position.x.0 as f64, e.position.y.0 as f64, 27.0, 27.0, 18.0);
        })
        .on_mouse_leave(move |_: &MouseEvent| {
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

/// A group member in the rail, by kind: a bot's plate, a chat's letter
/// tile, an artifact's tile (its group section rebuilds on any change).
fn rail_member(gid: String, m: GroupRow) -> AnyElement {
    match m {
        GroupRow::Bot(b) => rail_btn(signal(b).read_only()).into_element(),
        GroupRow::Chat(t) => rail_chat(signal(t).read_only()).into_element(),
        GroupRow::Artifact(a) => rail_art(gid, signal(a).read_only()).into_element(),
        GroupRow::App(key, title) => rail_app(gid, key, title).into_element(),
    }
}

/// A pinned mini app's tile: its title's initial on the title's hue,
/// rounded square as an app's (a chat's is round).
fn app_tile(title: &str, size: f32) -> impl IntoElement {
    view()
        .size(px(size))
        .rounded(px((size * 0.225).round()))
        .bg(hue(title))
        .items_center()
        .justify_center()
        .child(
            text(title.chars().find(|c| c.is_alphanumeric()).unwrap_or('?').to_uppercase().collect::<String>())
                .text_size(px((size * 0.4).round()))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(white()),
        )
}

/// A group's mini app in the rail: its tile; a press opens it full screen.
fn rail_app(gid: String, key: String, title: String) -> impl IntoElement {
    let hovered = signal(false);
    let (key2, title2) = (key.clone(), title.clone());
    view()
        .size(px(54.0))
        .rounded(px(10.0))
        .items_center()
        .justify_center()
        .bg(move || if hovered.get() { plate_hover() } else { AdaptiveColor::TRANSPARENT })
        .label(title.clone())
        .child(app_tile(&title, 36.0))
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .on_press(move || store().open_mini_full(&key, &title))
        .on_context_menu(move |e: &ContextMenuEvent| {
            crate::ctx::open(e, crate::ctx::group_app_items(&gid, &key2, &title2))
        })
}

/// A group's artifact in the rail: its tile on the rail's plate; a mini
/// app's hover shows its card; press opens it with its kind's handler.
fn rail_art(gid: String, a: ReadSignal<RoxApp>) -> impl IntoElement {
    let a0 = a.get_untracked();
    let hovered = signal(false);
    let node = element_ref();
    let node2 = node.clone();
    view()
        .element_ref(&node)
        .size(px(54.0))
        .rounded(px(10.0))
        .items_center()
        .justify_center()
        .bg(move || if hovered.get() { plate_hover() } else { AdaptiveColor::TRANSPARENT })
        .child(dynamic(move || crate::appdetails::artifact_tile(&a.get(), 36.0).into_element()))
        .on_mouse_enter(move |_: &MouseEvent| {
            hovered.set(true);
            let a = a.get_untracked();
            if a.kind != "app" {
                return;
            }
            let st = store();
            let seq = st.pop_seq.get() + 1;
            st.pop_seq.set(seq);
            let node = node2.clone();
            spawn(async move {
                if let Some(m) = node.measure().await {
                    let st = store();
                    if st.pop_seq.get() == seq {
                        st.rail_pop.set(Some(crate::store::RailPop {
                            bot: RoxBot {
                                name: a.title.clone(),
                                desc: if a.description.is_empty() {
                                    "Mini app".to_owned()
                                } else {
                                    a.description.clone()
                                },
                                ..Default::default()
                            },
                            cy: m.page.y.0 + m.frame.size.height.0 / 2.0,
                            x: m.page.x.0 + m.frame.size.width.0 + 6.0,
                            app: a.app.clone(),
                        }));
                    }
                }
            });
        })
        .on_mouse_leave(move |_: &MouseEvent| {
            hovered.set(false);
            let st = store();
            st.pop_seq.set(st.pop_seq.get() + 1);
            st.rail_pop.set(None);
        })
        .on_press(move || store().open_library_item(&a.get_untracked()))
        .on_context_menu(move |e: &ContextMenuEvent| {
            crate::ctx::open(e, crate::ctx::group_art_items(&gid, &a.get()))
        })
        .label(a0.title)
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
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .on_press(move || store().open_thread(id.clone()))
        .on_context_menu(move |e: &ContextMenuEvent| {
            crate::ctx::open(e, crate::ctx::chat_items(&t.get()))
        })
        .label(name)
}

fn full_sidebar() -> impl IntoElement {
    let fade_top = signal(false);
    let fade_bottom = signal(true);
    // Rows dragged into groups: the list's drag state, for its rows.
    let dd = Dnd::install();
    let (dd1, dd2, dd3, dd4, dd5, dd6, dd7) =
        (dd.clone(), dd.clone(), dd.clone(), dd.clone(), dd.clone(), dd.clone(), dd.clone());
    // Rows that load above the visible ones (the pinned tiles after the
    // chats) left the list opened partway down: once the bots are in, it
    // goes back to the top (once, a moment after launch).
    let list = element_ref();
    {
        let list = list.clone();
        let done = std::cell::Cell::new(false);
        effect(move || {
            let loaded = !store().bots.with(|b| b.is_empty());
            if loaded && !done.replace(true) {
                let list = list.clone();
                spawn(async move {
                    timer(std::time::Duration::from_millis(120)).await;
                    list.scroll_to(Point { x: px(0.0), y: px(0.0) }, false);
                });
            }
        });
    }
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
                // Nothing of the list draws past its edges: rows ran on
                // under the footer (Grok Bot's list ends at its top).
                .overflow_hidden()
                // Grok Bot's list fades (`--scroll-fade-top-size: 28px`,
                // `--scroll-fade-bottom-size: 48px`): the top one only once
                // the list is scrolled past its 4pt padding — a tile at rest
                // is never washed — the bottom one while more is below.
                .on_layout(move |b: &Bounds<Points>| dd1.view_h.set(b.size.height.0))
                .child(blur_edge(true, 28.0, fade_top))
                .child(blur_edge(false, 48.0, fade_bottom))
                .child(drag_ghost(dd))
                .child(
                    scroll()
                        .flex_1()
                        .element_ref(&list)
                        .on_scroll(move |e: &ScrollEvent| {
                            let y = e.offset.y.0;
                            dd2.scroll_y.set(y);
                            let top = y > 4.0;
                            let bottom = y + e.size.height.0 < e.content_size.height.0 - 1.0;
                            if fade_top.get_untracked() != top {
                                fade_top.set(top);
                            }
                            if fade_bottom.get_untracked() != bottom {
                                fade_bottom.set(bottom);
                            }
                        })
                        .child(
                        view()
                            // A drag starts where the pointer last hovered
                            // (pressed, it stops hovering); its moves pick
                            // the drop target, in this view's coordinates.
                            .on_layout(move |b: &Bounds<Points>| dd3.width.set(b.size.width.0))
                            .on_mouse_move(move |e: &MouseEvent| {
                                let at = (e.position.x.0, e.position.y.0);
                                if e.pressed_button.is_none() {
                                    dd4.down.set(at);
                                }
                                dd5.follow(at);
                            })
                            // Grok Bot's: the first tile 10pt below the
                            // list's top (4 here + the tile row's 6).
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
                    // The user's groups — bots, chats and artifacts, each
                    // group a collapsible block under its colored header.
                    .child(each(
                        || store().groups.get(),
                        |g| g.id.clone(),
                        |g, _| group_block(g),
                    ))
                    // The visible, unpinned bots no group holds.
                    .child(when(
                        || !store().ungrouped_bots().is_empty(),
                        move || {
                            let dd = dd6.clone();
                            view()
                                .on_layout(move |b: &Bounds<Points>| dd.zone("__un", b))
                                .child(sec_header(
                                    "__un".to_owned(),
                                    || store().unassigned_name.get(),
                                    Some(std::rc::Rc::new(|| store().ungrouped_bots().len())),
                                ))
                                .child(when(
                                    || !collapsed("__un"),
                                    || {
                                        each(
                                            || store().ungrouped_bots(),
                                            |b| b.id.clone(),
                                            |b, _| bot_row(b, Src::list("__un")),
                                        )
                                    },
                                ))
                        },
                    ))
                    // `chats` — the plain list; bot threads live behind
                    // their persona rows.
                    .child(
                        view()
                            .on_layout(move |b: &Bounds<Points>| dd7.zone("__chats", b))
                            .child(sec_header(
                                "__chats".to_owned(),
                                || store().chats_name.get(),
                                Some(std::rc::Rc::new(|| store().chats().len())),
                            )),
                    )
                    .child(when(
                        || !collapsed("__chats"),
                        || {
                            each(
                                || store().chats(),
                                |t| t.id.clone(),
                                |t, _| thread_row(t, Src::list("__chats")),
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
                                    || "Hidden Bots".to_owned(),
                                    Some(std::rc::Rc::new(|| store().hidden_bots().len())),
                                ))
                                .child(when(
                                    || !collapsed("__hidden"),
                                    || {
                                        each(
                                            || store().hidden_bots(),
                                            |b| b.id.clone(),
                                            |b, _| bot_row(b, None),
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
                                    || "Hidden Chats".to_owned(),
                                    Some(std::rc::Rc::new(|| store().hidden_chats().len())),
                                ))
                                .child(when(
                                    || !collapsed("__hiddenChats"),
                                    || {
                                        each(
                                            || store().hidden_chats(),
                                            |t| t.id.clone(),
                                            |t, _| thread_row(t, None),
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
                                    || "Archived".to_owned(),
                                    Some(std::rc::Rc::new(|| store().archived_chats().len())),
                                ))
                                .child(when(
                                    || !collapsed("__archived"),
                                    || {
                                        each(
                                            || store().archived_chats(),
                                            |t| t.id.clone(),
                                            |t, _| thread_row(t, None),
                                        )
                                    },
                                ))
                        },
                    )),
                ),
            ),
        )
        // The footer, Grok Bot's: the account pill fills the row; hovering
        // the connect pill folds the account to its circle and spells out
        // "Connect apps" beside the logos.
        .child({
            let connect = signal(false);
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .px_4()
                .pb_4()
                .child(account_pill(connect))
                .child(connect_pill(connect))
        })
}

/// The rail's account plate, Grok Bot's: the 36pt circle on a 54pt
/// plate (radius 10) that takes the 4% fill while hovered.
fn rail_account() -> impl IntoElement {
    let hovered = signal(false);
    view()
        .size(px(54.0))
        .rounded(px(10.0))
        .items_center()
        .justify_center()
        .bg(move || if hovered.get() { rail_hover() } else { AdaptiveColor::TRANSPARENT })
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .on_press(|| store().account_open.update(|v| *v = !*v))
        .child(account_circle())
}

/// The account's name: the macOS full name (`id -F`), else `$USER`.
fn account_name() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        std::process::Command::new("id")
            .arg("-F")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty())
            .or_else(|| std::env::var("USER").ok())
            .unwrap_or_else(|| "You".to_owned())
    })
}

/// Grok Bot's initials: the first letter of each of the first two words
/// ("Uriah Galang" → "UG", "uriah" → "U").
fn account_initials() -> String {
    account_name()
        .split_whitespace()
        .take(2)
        .filter_map(|w| w.chars().next())
        .flat_map(char::to_uppercase)
        .collect()
}

/// The account avatar, Grok Bot's `kit-base-avatar`: a 36pt disc at 8%
/// ink with a 0.5pt 6% rim, the initials 14px/500 at 66% ink.
fn account_circle() -> impl IntoElement {
    view()
        .size(px(36.0))
        .rounded(px(18.0))
        .flex_shrink(0.0)
        .bg(theme::tc("#f5f5f514"))
        .border(px(0.5))
        .border_color(theme::tc("#f5f5f50f"))
        .items_center()
        .justify_center()
        .child(
            text(account_initials())
                .text_size(px(14.0))
                .font_weight(FontWeight::MEDIUM)
                .letter_spacing(px(-0.35))
                .text_color(theme::tc("#f5f5f5a8")),
        )
}

/// The account pill (→ popover): the circle and the name on the card
/// fill, growing into the row; the name ellipsizes to the room the
/// connect pill leaves, and drops out while that pill is hovered.
fn account_pill(connect: Signal<bool>) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .h(px(36.0))
        .min_w(px(36.0))
        .flex_grow(move || if connect.get() { 0.0f32 } else { 1.0 })
        .flex_shrink(1.0)
        .rounded(px(18.0))
        .overflow_hidden()
        .bg(move || if store().account_open.get() { theme::CARD2 } else { theme::CARD })
        .child(account_circle())
        .child(when(
            move || !connect.get(),
            || {
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .ml(px(8.0))
                    .mr(px(17.0))
                    .child(
                        text(account_name())
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::TEXT)
                            .number_of_lines(1),
                    )
                    .into_element()
            },
        ))
        .on_press(|| store().account_open.update(|v| *v = !*v))
        .label("Account")
}

/// The connect pill, Grok Bot's: the connected apps' logos stacked 12pt
/// apart (each on an 18pt card ring); hovered it takes the row and leads
/// with "Connect apps", keeping the logos that still fit.
fn connect_pill(hovered: Signal<bool>) -> impl IntoElement {
    const LABEL: &str = "Connect apps";
    // The row is the 240pt sidebar less its 0.5 rim and 16pt gutters.
    const ROW: f32 = 240.0 - 0.5 - 32.0;
    let label_w = measure_text("500 14px -apple-system, system-ui, sans-serif", LABEL).0.ceil();
    // Hovered: row − gap − circle − border − 16/12 padding − label − 6.
    let room = ROW - 8.0 - 36.0 - 2.0 - 16.0 - 12.0 - label_w - 6.0;
    let fits = ((room - 18.0) / 12.0).floor().max(0.0) as usize + 1;
    let apps = move || -> Vec<String> {
        store()
            .plugins
            .get()
            .iter()
            .filter(|p| p.installed)
            .take(3)
            .map(|p| p.name.clone())
            .collect()
    };
    view()
        .flex_row()
        .items_center()
        .h(px(36.0))
        .min_w(px(36.0))
        .flex_grow(move || if hovered.get() { 1.0f32 } else { 0.0 })
        .flex_shrink(0.0)
        .px(px(12.0))
        .rounded(px(18.0))
        .bg(theme::CARD)
        .border(px(1.0))
        .border_color(theme::tc("#f5f5f50f"))
        .child(when(
            move || hovered.get(),
            move || {
                view()
                    .flex_row()
                    .items_center()
                    .ml(px(4.0))
                    .mr(px(6.0))
                    .child(
                        text(LABEL)
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::TEXT)
                            .number_of_lines(1),
                    )
                    .into_element()
            },
        ))
        .child(dynamic(move || {
            let mut names = apps();
            if names.is_empty() {
                return icon_sized("grokGridPlus", 16.0).into_element();
            }
            if hovered.get() {
                names.truncate(fits);
            }
            let n = names.len();
            names
                .into_iter()
                .enumerate()
                .fold(
                    view().w(px(18.0 + 12.0 * (n - 1) as f32)).h(px(18.0)),
                    |stack, (i, name)| {
                        stack.child(
                            view()
                                .absolute()
                                .left(px(12.0 * i as f32))
                                .top(px(0.0))
                                .size(px(18.0))
                                .rounded(px(6.0))
                                .bg(theme::CARD)
                                .items_center()
                                .justify_center()
                                .child(crate::plugins::plug_tile(&name, 16.0)),
                        )
                    },
                )
                .into_element()
        }))
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .on_press(|| store().open_plugins())
        .label(LABEL)
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
                .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
                .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
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
                                        text(account_initials())
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme::TEXT),
                                    ),
                            )
                            .child(
                                view()
                                    .flex_1()
                                    .child(
                                        text(account_name())
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

// --- Dragging rows into groups ------------------------------------------------------

/// Where a draggable row sits: the bucket its frame is kept under (its
/// group's id, `__un`, `__chats`) and whether that is a group.
#[derive(Clone)]
struct Src {
    zone: String,
    group: bool,
}

impl Src {
    fn group(gid: &str) -> Option<Self> {
        Some(Self { zone: gid.to_owned(), group: true })
    }

    fn list(zone: &str) -> Option<Self> {
        Some(Self { zone: zone.to_owned(), group: false })
    }
}

/// A row being dragged: the item, the group it came out of, and its
/// ghost — face, title and where it starts in the list's viewport.
#[derive(Clone, PartialEq)]
struct Dragged {
    kind: &'static str,
    id: String,
    thread: String,
    from: Option<String>,
    title: String,
    face: Face,
    at: (f32, f32),
}

impl Dragged {
    fn new(kind: &'static str, id: String, thread: String, title: String, face: Face) -> Self {
        Self { kind, id, thread, from: None, title, face, at: (0.0, 0.0) }
    }
}

/// The ghost's icon, by kind.
#[derive(Clone, PartialEq)]
enum Face {
    Bot(RoxBot),
    Chat(String),
    Art(RoxApp),
}

/// Where a drop lands: group `gid` before its visible row `slot` (its
/// end on the header, `None`), or out of the source group (on "Bots
/// without a group" `__un`, or Chats `__chats`).
#[derive(Clone, PartialEq)]
enum Target {
    Into(String, Option<usize>),
    Out(&'static str),
}

/// The drag state of the full sidebar's list. Coordinates are the list
/// content's; the frames are kept as they lay out (no signal), and only
/// the dragged row and the target are signals, so a drag moves the ghost
/// natively (`dx`/`dy`, the rows' pan values) and redraws the header
/// and insertion line it is over, nothing else.
struct Dnd {
    dragged: Signal<Option<Dragged>>,
    target: Signal<Option<Target>>,
    dx: AnimatedValue,
    dy: AnimatedValue,
    /// The pointer when it last hovered, unpressed: where a drag began.
    down: std::cell::Cell<(f32, f32)>,
    scroll_y: std::cell::Cell<f32>,
    view_h: std::cell::Cell<f32>,
    width: std::cell::Cell<f32>,
    /// Group blocks, `__un` and the Chats header: (y, height).
    zones: std::cell::RefCell<std::collections::HashMap<String, (f32, f32)>>,
    /// Rows by zone and `GroupRow::key`: (y in their parent, height).
    rows: std::cell::RefCell<std::collections::HashMap<String, std::collections::HashMap<String, (f32, f32)>>>,
    /// Where a group's rows' parent starts inside its block (below the
    /// header): row frames are relative to it.
    bases: std::cell::RefCell<std::collections::HashMap<String, f32>>,
}

thread_local! {
    static DND: std::cell::RefCell<Option<std::rc::Rc<Dnd>>> = const { std::cell::RefCell::new(None) };
}

fn dnd() -> Option<std::rc::Rc<Dnd>> {
    DND.with(|d| d.borrow().clone())
}

impl Dnd {
    fn install() -> std::rc::Rc<Self> {
        let d = std::rc::Rc::new(Self {
            dragged: signal(None),
            target: signal(None),
            dx: animated_value(0.0),
            dy: animated_value(0.0),
            down: Default::default(),
            scroll_y: Default::default(),
            view_h: Default::default(),
            width: Default::default(),
            zones: Default::default(),
            rows: Default::default(),
            bases: Default::default(),
        });
        DND.with(|s| *s.borrow_mut() = Some(d.clone()));
        d
    }

    fn zone(&self, key: &str, b: &Bounds<Points>) {
        self.zones.borrow_mut().insert(key.to_owned(), (b.origin.y.0, b.size.height.0));
    }

    /// The drag past the slop: the ghost shows at the press, offset to
    /// the pointer's right, and follows the pan's translation.
    fn start(&self, src: &Src, mut d: Dragged) {
        let (x, y) = self.down.get();
        d.from = src.group.then(|| src.zone.clone());
        d.at = (x + 14.0, y - self.scroll_y.get() - 13.0);
        self.target.set(None);
        self.dragged.set(Some(d));
    }

    /// The pointer moved during a drag: the target, when it changed.
    fn follow(&self, p: (f32, f32)) {
        let Some(t) = self.dragged.with_untracked(|d| d.as_ref().map(|d| self.target_at(d, p))) else {
            return;
        };
        if self.target.with_untracked(|c| *c != t) {
            self.target.set(t);
        }
    }

    /// The drop, where the pan ended (off the list it cancels — the list
    /// hears no moves there): into a group at its slot (a copy from
    /// another group, a reorder in its own), or out of the source group.
    fn land(&self, e: &PanEvent) {
        let Some(d) = self.dragged.get_untracked() else { return };
        let (x, y) = self.down.get();
        let t = self.target_at(&d, (x + e.translation.x.0, y + e.translation.y.0));
        self.dragged.set(None);
        self.target.set(None);
        self.dx.set(0.0);
        self.dy.set(0.0);
        let st = store();
        match t {
            Some(Target::Into(gid, slot)) => {
                let index = slot.map(|i| self.index_of(&gid, &d, i));
                if d.from.as_deref() == Some(gid.as_str()) {
                    // where it already is: nothing to do
                    let (at, n) = st.groups.with_untracked(|gs| {
                        let items = gs.iter().find(|g| g.id == gid).map(|g| &g.items[..]).unwrap_or(&[]);
                        (items.iter().position(|it| it.kind == d.kind && it.id == d.id), items.len())
                    });
                    if at.is_some() && at == Some(index.unwrap_or(n.saturating_sub(1))) {
                        return;
                    }
                }
                st.group_place(&gid, d.kind, &d.id, &d.thread, index, None);
            }
            Some(Target::Out(_)) => {
                if let Some(from) = &d.from {
                    st.group_item(from, d.kind, &d.id, "", false);
                }
            }
            None => {}
        }
    }

    /// Visible slot `slot` of group `gid` as an index into its items
    /// without the dragged one (hidden members count there too).
    fn index_of(&self, gid: &str, d: &Dragged, slot: usize) -> usize {
        let me = format!("{}:{}", d.kind, d.id);
        untracked(|| {
            let keys: Vec<String> = store().group_rows(gid).iter().map(GroupRow::key).collect();
            let anchor = keys.get(slot..).and_then(|ks| ks.iter().find(|k| **k != me).cloned());
            store().groups.with(|gs| {
                let items: Vec<String> = gs
                    .iter()
                    .find(|g| g.id == gid)
                    .map(|g| g.items.iter().map(|it| format!("{}:{}", it.kind, it.id)).filter(|k| *k != me).collect())
                    .unwrap_or_default();
                anchor.and_then(|a| items.iter().position(|k| *k == a)).unwrap_or(items.len())
            })
        })
    }

    /// What is under list point (`x`, `y`) for `d`: a group's header
    /// (its end — a folded group takes drops only there), the slot between
    /// its rows nearest the pointer, or — for a row out of a group — "Bots
    /// without a group" and Chats.
    fn target_at(&self, d: &Dragged, (x, y): (f32, f32)) -> Option<Target> {
        let vy = y - self.scroll_y.get();
        if x < 0.0 || x > self.width.get() || vy < 0.0 || vy > self.view_h.get() {
            return None;
        }
        untracked(|| {
            let zones = self.zones.borrow();
            let rows = self.rows.borrow();
            // the list's 10pt gaps split between the blocks around them
            let hit = |k: &str| zones.get(k).is_some_and(|&(zy, zh)| y >= zy - 5.0 && y < zy + zh + 5.0);
            for g in store().groups.get() {
                if !hit(&g.id) {
                    continue;
                }
                let zy = zones[&g.id].0;
                if y < zy + 30.0 || collapsed(&g.id) {
                    return Some(Target::Into(g.id, None));
                }
                let frames = rows.get(&g.id);
                let slot = store()
                    .group_rows(&g.id)
                    .iter()
                    .filter(|r| {
                        frames
                            .and_then(|f| f.get(&r.key()))
                            .is_some_and(|&(ry, rh)| self.base(&g.id) + ry + rh / 2.0 < y - zy)
                    })
                    .count();
                return Some(Target::Into(g.id, Some(slot)));
            }
            d.from.as_ref()?;
            if hit("__un") {
                return Some(Target::Out("__un"));
            }
            // Chats: its header down to its last row
            let (cy, ch) = *zones.get("__chats")?;
            let mut end = cy + ch;
            if !collapsed("__chats") {
                if let Some(f) = rows.get("__chats") {
                    for t in store().chats() {
                        if let Some(&(ry, rh)) = f.get(&format!("chat:{}", t.id)) {
                            end = end.max(ry + rh);
                        }
                    }
                }
            }
            (y >= cy - 5.0 && y < end + 5.0).then_some(Target::Out("__chats"))
        })
    }

    /// The insertion line's y in group `gid`'s block while a drop would
    /// land in it (tracks the target): above the slot's row, or under
    /// the last for the end. None in a folded or empty group.
    fn line_y(&self, gid: &str) -> Option<f32> {
        let slot = self.target.with(|t| match t {
            Some(Target::Into(g, s)) if g == gid => Some(*s),
            _ => None,
        })?;
        untracked(|| {
            if collapsed(gid) {
                return None;
            }
            let rows = self.rows.borrow();
            let f = rows.get(gid)?;
            let base = self.base(gid);
            let frames: Vec<(f32, f32)> = store()
                .group_rows(gid)
                .iter()
                .filter_map(|r| f.get(&r.key()).map(|&(y, h)| (base + y, h)))
                .collect();
            match slot.and_then(|i| frames.get(i)) {
                Some(&(y, _)) => Some(y),
                None => frames.last().map(|&(y, h)| y + h),
            }
        })
    }

    /// The offset of group `gid`'s rows inside its block.
    fn base(&self, gid: &str) -> f32 {
        self.bases.borrow().get(gid).copied().unwrap_or(0.0)
    }

    /// Whether the drop would land in group `gid` (its header lights).
    fn over_group(&self, gid: &str) -> bool {
        self.target.with(|t| matches!(t, Some(Target::Into(g, _)) if g == gid))
    }

    /// Whether the drop would take the row out onto section `key`.
    fn over_section(&self, key: &str) -> bool {
        self.target.with(|t| matches!(t, Some(Target::Out(k)) if *k == key))
    }
}

/// Makes `row` a drag source (with `src`; none for rows that aren't):
/// its frame is kept for the drop slots, and a drag past nsh's 6pt slop
/// carries `item()` — a shorter move stays a press, a right-click its
/// menu.
fn draggable(row: View, src: Option<Src>, key: String, item: impl Fn() -> Dragged + 'static) -> View {
    let (Some(src), Some(d)) = (src, dnd()) else { return row };
    let zone = src.zone.clone();
    let (d2, d3) = (d.clone(), d.clone());
    row.on_layout(move |b: &Bounds<Points>| {
        d.rows
            .borrow_mut()
            .entry(zone.clone())
            .or_default()
            .insert(key.clone(), (b.origin.y.0, b.size.height.0));
    })
    .pan_values(Some(&d2.dx), Some(&d2.dy))
    .on_pan_start(move |_: &PanEvent| d2.start(&src, item()))
    .on_pan_end(move |e: &PanEvent| d3.land(e))
}

/// The dragged row's ghost: its icon and title in a pill at the pointer,
/// moved natively by the pan's values (made once per drag).
fn drag_ghost(d: std::rc::Rc<Dnd>) -> impl IntoElement {
    let d2 = d.clone();
    when(
        move || d.dragged.with(Option::is_some),
        move || {
            let Some(g) = d2.dragged.get_untracked() else {
                return view().into_element();
            };
            let face = match &g.face {
                Face::Bot(b) => bot_avatar(crate::avatar::BotAvatarProps::bot(b, 20.0)).into_element(),
                Face::Chat(name) => view()
                    .size(px(20.0))
                    .rounded(px(10.0))
                    .bg(hue(name))
                    .items_center()
                    .justify_center()
                    .child(
                        text(name.chars().next().unwrap_or('?').to_uppercase().collect::<String>())
                            .text_size(px(10.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(white()),
                    )
                    .into_element(),
                Face::Art(a) => crate::appdetails::artifact_tile(a, 20.0).into_element(),
            };
            view()
                .absolute()
                .left(px(g.at.0))
                .top(px(g.at.1))
                .z_index(4)
                .pointer_events(PointerEvents::None)
                .bind_translate_x(&d2.dx)
                .bind_translate_y(&d2.dy)
                .max_w(px(180.0))
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .pl(px(4.0))
                .pr(px(10.0))
                .py(px(3.0))
                .rounded(px(13.0))
                .bg(theme::PILL_BG)
                .border(px(0.5))
                .border_color(theme::BORDER_S)
                .child(face)
                .child(
                    text(g.title.clone())
                        .text_size(px(12.0))
                        .text_color(theme::TEXT)
                        .number_of_lines(1),
                )
                .into_element()
        },
    )
}

/// `secHeader` — a tappable section title; the chevron appears on hover
/// and the count sits beside it (`toggleSec` on press).
fn sec_header(
    key: String,
    title: impl Fn() -> String + 'static,
    count: Option<std::rc::Rc<dyn Fn() -> usize>>,
) -> impl IntoElement {
    let key2 = key.clone();
    let key3 = key.clone();
    let key4 = key.clone();
    let hovered = signal(false);
    let drop_on = dnd();
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
            // a row dragged out of its group, over this section
            if drop_on.as_ref().is_some_and(|d| d.over_section(&key)) {
                theme::ACCENT_TINT
            } else if hovered.get() {
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
        // Unassigned and Chats are renamable (their menu: Rename…, fold)
        .on_context_menu(move |e: &ContextMenuEvent| {
            if key4 == "__un" || key4 == "__chats" {
                crate::ctx::open(e, crate::ctx::section_header_items(&key4));
            }
        })
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
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .on_press(move || store().toggle_section(&key2))
}

/// A group: its header, then — unless folded — its members, each in its
/// kind's row look.
fn group_block(g: ReadSignal<RoxGroup>) -> impl IntoElement {
    let gid = g.get_untracked().id;
    let (gid2, gid3) = (gid.clone(), gid.clone());
    let block = view().child(group_header(g)).child(when(
        move || !collapsed(&gid),
        move || {
            // Rebuilt whole when the members change (a group is a handful
            // of rows): a keyed `each` here lost kept rows after a reorder
            // ("a kept row is gone"), leaving an open group empty.
            let gid = gid2.clone();
            dynamic(move || {
                let at = gid.clone();
                view()
                    .on_layout(move |b: &Bounds<Points>| {
                        if let Some(d) = dnd() {
                            d.bases.borrow_mut().insert(at.clone(), b.origin.y.0);
                        }
                    })
                    .children(
                        store()
                            .group_rows(&gid)
                            .into_iter()
                            .map(|m| group_member(gid.clone(), signal(m).read_only())),
                    )
                    .into_element()
            })
        },
    ));
    let Some(d) = dnd() else { return block };
    // A drag over it: the accent line where the row would land.
    let (d2, d3, gid4) = (d.clone(), d.clone(), gid3.clone());
    block
        .on_layout(move |b: &Bounds<Points>| d.zone(&gid3, b))
        .child(when(
            {
                let gid = gid4.clone();
                move || d2.line_y(&gid).is_some()
            },
            move || {
                let (d, gid) = (d3.clone(), gid4.clone());
                view()
                    .absolute()
                    .left(px(8.0))
                    .right(px(8.0))
                    .h(px(2.0))
                    .rounded(px(1.0))
                    .bg(theme::ACCENT)
                    .pointer_events(PointerEvents::None)
                    .top(move || px(d.line_y(&gid).unwrap_or(0.0) - 1.0))
                    .into_element()
            },
        ))
}

/// A group's header: `secHeader`'s row with the group's color dot, its
/// name and member count; a press folds it, the context menu renames,
/// recolors, folds or deletes it.
fn group_header(g: ReadSignal<RoxGroup>) -> impl IntoElement {
    let gid = g.get_untracked().id;
    let (gid2, gid3, gid4) = (gid.clone(), gid.clone(), gid.clone());
    let hovered = signal(false);
    let drop_on = dnd();
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
            // a drag would drop into this group
            if drop_on.as_ref().is_some_and(|d| d.over_group(&gid4)) {
                theme::ACCENT_TINT
            } else if hovered.get() {
                theme::HOVER
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .child(
            view()
                .size(px(8.0))
                .rounded(px(4.0))
                .bg(move || Hsla::from_str(&g.get().color).unwrap_or(theme::MUTED.light)),
        )
        .child(
            text(move || g.get().name)
                .text_size(px(12.0))
                .text_color(theme::MUTED)
                .flex_1()
                .number_of_lines(1),
        )
        .child(
            text(move || store().group_rows(&gid).len().to_string())
                .text_size(px(12.0))
                .text_color(theme::MUTED),
        )
        .child(
            text(move || if collapsed(&gid2) { "›" } else { "⌄" })
                .text_size(px(13.0))
                .text_color(theme::MUTED)
                .opacity(move || if hovered.get() { 1.0f32 } else { 0.0f32 }),
        )
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .on_press(move || store().toggle_section(&gid3))
        .on_context_menu(move |e: &ContextMenuEvent| {
            crate::ctx::open(e, crate::ctx::group_header_items(&g.get()))
        })
}

/// A group member in its kind's row: a bot's, a chat's, an artifact's.
fn group_member(gid: String, m: ReadSignal<GroupRow>) -> AnyElement {
    match m.get_untracked() {
        GroupRow::Bot(_) => each(
            move || match m.get() {
                GroupRow::Bot(b) => vec![b],
                _ => Vec::new(),
            },
            |b| b.id.clone(),
            {
                let gid = gid.clone();
                move |b, _| bot_row(b, Src::group(&gid))
            },
        ),
        GroupRow::Chat(_) => each(
            move || match m.get() {
                GroupRow::Chat(t) => vec![t],
                _ => Vec::new(),
            },
            |t| t.id.clone(),
            {
                let gid = gid.clone();
                move |t, _| thread_row(t, Src::group(&gid))
            },
        ),
        GroupRow::Artifact(_) => each(
            move || match m.get() {
                GroupRow::Artifact(a) => vec![a],
                _ => Vec::new(),
            },
            |a| a.app.clone(),
            move |a, _| group_art_row(gid.clone(), a),
        ),
        GroupRow::App(key, title) => group_app_row(gid, key, title).into_element(),
    }
}

/// A group's mini app (a pinned `rox://app/` link): its tile, its title
/// and its server; a press opens it full screen.
fn group_app_row(gid: String, key: String, title: String) -> impl IntoElement {
    let hovered = signal(false);
    let gid2 = gid.clone();
    let row_key = format!("app:{key}");
    let server = key.trim_start_matches("mcp:").split('/').next().unwrap_or("").to_owned();
    let (key2, key3, title2, title3) = (key.clone(), key.clone(), title.clone(), title.clone());
    let row = view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .py(px(7.0))
        .rounded(px(8.0))
        .bg(move || if hovered.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .on_press(move || store().open_mini_full(&key, &title))
        .on_context_menu(move |e: &ContextMenuEvent| {
            crate::ctx::open(e, crate::ctx::group_app_items(&gid, &key2, &title2))
        })
        .child(app_tile(&title3, 36.0))
        .child(
            view()
                .flex_1()
                .child(text(title3.clone()).text_size(px(13.0)).text_color(theme::TEXT).number_of_lines(1))
                .child(
                    text(format!("Mini app · {server}"))
                        .text_size(px(12.0))
                        .text_color(theme::MUTED)
                        .number_of_lines(1),
                ),
        );
    let src = Src::group(&gid2);
    // its title rides where an artifact's origin chat would
    draggable(row, src, row_key, move || {
        Dragged::new("app", key3.clone(), title3.clone(), title3.clone(), Face::Chat(title3.clone()))
    })
}

/// A group's artifact row: its kind's tile (an app's icon, an image's
/// picture, a file's type tile), its title and the kind's subtitle; a
/// press opens it with its kind's handler from whatever chat is open.
fn group_art_row(gid: String, a: ReadSignal<RoxApp>) -> impl IntoElement {
    let hovered = signal(false);
    let gid2 = gid.clone();
    let key = format!("artifact:{}", a.get_untracked().app);
    let row = view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .py(px(7.0))
        .rounded(px(8.0))
        .bg(move || if hovered.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .on_press(move || store().open_library_item(&a.get_untracked()))
        .on_context_menu(move |e: &ContextMenuEvent| {
            crate::ctx::open(e, crate::ctx::group_art_items(&gid, &a.get()))
        })
        .child(dynamic(move || crate::appdetails::artifact_tile(&a.get(), 36.0).into_element()))
        .child(
            view()
                .flex_1()
                .child(
                    text(move || a.get().title)
                        .text_size(px(13.0))
                        .text_color(theme::TEXT)
                        .number_of_lines(1),
                )
                .child(
                    text(move || crate::pane::subtitle(&a.get()))
                        .text_size(px(12.0))
                        .text_color(theme::MUTED)
                        .number_of_lines(1),
                ),
        );
    let src = Src::group(&gid2);
    draggable(row, src, key, move || {
        let a = a.get_untracked();
        // its origin chat goes along, for the group it lands in
        let thread = store().groups.with_untracked(|gs| {
            gs.iter()
                .find(|g| g.id == gid2)
                .and_then(|g| g.items.iter().find(|it| it.kind == "artifact" && it.id == a.app))
                .map(|it| it.thread.clone())
                .unwrap_or_default()
        });
        Dragged::new("artifact", a.app.clone(), thread, a.title.clone(), Face::Art(a))
    })
}

/// A chat's row; `src` makes it draggable into groups (and says which
/// group it shows in).
fn thread_row(t: ReadSignal<ThreadMeta>, src: Option<Src>) -> impl IntoElement {
    let st = store();
    let id = t.get().id.clone();
    let tid = t.get().id.clone();
    let hovered = signal(false);
    let active = move || st.active_id.get() == tid;
    let key = format!("chat:{id}");
    let in_group = src.as_ref().filter(|s| s.group).map(|s| s.zone.clone());
    let row = view()
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
                                // a v1 token in the title shows its ticket's title (9.3)
                                if v.name.is_empty() { "New chat".to_owned() } else { store().v1_plain(&v.name) }
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
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hovered.set(false))
        .on_press(move || st.open_thread(id.clone()))
        .on_context_menu(move |e: &ContextMenuEvent| {
            let t = t.get();
            let items = crate::ctx::chat_items(&t);
            crate::ctx::open(e, match &in_group {
                Some(gid) => crate::ctx::with_move(items, gid, "chat", &t.id, ""),
                None => items,
            })
        });
    draggable(row, src, key, move || {
        let t = t.get_untracked();
        let title = if t.name.is_empty() { "New chat".to_owned() } else { t.name.clone() };
        Dragged::new("chat", t.id, String::new(), title, Face::Chat(t.name))
    })
}

/// A bot's row; `src` makes it draggable into groups (and says which
/// group it shows in).
fn bot_row(b: ReadSignal<RoxBot>, src: Option<Src>) -> impl IntoElement {
    let hovered = signal(false);
    let b0 = b.get();
    let key = format!("bot:{}", b0.id);
    let in_group = src.as_ref().filter(|s| s.group).map(|s| s.zone.clone());
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
    let row = view()
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
        .on_mouse_enter(move |_: &MouseEvent| hovered.set(true))
        .on_mouse_move(move |e: &MouseEvent| {
            // `railGaze(...).follow(e, 26, 27, 18)` — the 36px cell's center
            // in row coordinates: here 12px pad + 18px half-cell = 30, and
            // the row half-height is (7 + 36 + 7) / 2 = 25.
            gaze.follow(e.position.x.0 as f64, e.position.y.0 as f64, 30.0, 25.0, 18.0);
        })
        .on_mouse_leave(move |_: &MouseEvent| {
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
            let b = b.get();
            let items = crate::ctx::bot_items(&b);
            crate::ctx::open(e, match &in_group {
                Some(gid) => crate::ctx::with_move(items, gid, "bot", &b.id, ""),
                None => items,
            })
        });
    draggable(row, src, key, move || {
        let b = b.get_untracked();
        Dragged::new("bot", b.id.clone(), String::new(), b.name.clone(), Face::Bot(b))
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
        // Grows with a label under the name (Grok Bot's tile); 92pt is
        // the tile without one.
        .min_h(px(92.0))
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
        .on_mouse_enter(move |_: &MouseEvent| {
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
                            app: String::new(),
                        }));
                    }
                }
            });
        })
        .on_mouse_move(move |e: &MouseEvent| {
            // `railGaze(b.id+":pin").follow(e, 47.5, 36, 30)` — the tile's
            // center (95/2=47.5) and mark row's center (~36px from top).
            gaze.follow(e.position.x.0 as f64, e.position.y.0 as f64, 47.5, 36.0, 30.0);
        })
        .on_mouse_leave(move |_: &MouseEvent| {
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
                            .child(if pop.app.is_empty() {
                                bot_avatar(crate::avatar::BotAvatarProps {
                                    size: 20.0,
                                    box_: Some(16.0),
                                    paused: true,
                                    ..crate::avatar::BotAvatarProps::bot(&pop.bot, 20.0)
                                })
                                .into_element()
                            } else {
                                crate::appdetails::app_icon(&pop.app, 16.0).into_element()
                            }),
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
