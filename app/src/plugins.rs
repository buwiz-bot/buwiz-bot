//! The plugins marketplace modal — Grok Bot's Connect-apps dialog ported
//! (App.tsx:3485–4188): a nav-stack card (market → "N connected" manager →
//! plugin / skill / mcp detail pages) over a 70% scrim. 800×702 max,
//! radius 14, `#1f1f1f` surface.
//!
//! Layout constants — `MK` (App.tsx:1367).
const MK_BAND: f32 = 48.0;
const MK_TITLE: f32 = 42.0;
const MK_LEFT: f32 = 33.0;
const MK_RIGHT: f32 = 48.0;
const MK_HEAD: f32 = 67.0;
const CAT_CAP: usize = 5;
const DROP_ROW: f32 = 40.0;
const DROP_HEAD: f32 = 30.0;
const DROP_MAX: f32 = 330.0;

use std::str::FromStr;

use nsh_base::input;
use nsh_core::prelude::*;

use crate::icons::icon;
use crate::live::store;
use crate::store::{McpServer, PluginNav, PluginRow, Store};
use crate::theme;

/// `tc("#f5f5f5<a>")` — ink wash at an alpha, both themes.
fn tc(a: f32) -> AdaptiveColor {
    theme::tone(theme::hsla(0.0, 0.0, 0.11, a), theme::hsla(0.0, 0.0, 0.96, a))
}

fn hx(hex: &str) -> AdaptiveColor {
    AdaptiveColor::from(Hsla::from_str(hex).unwrap_or(theme::ACCENT.dark))
}

/// `hue(name)` — the letter-tile color (App.tsx:102).
pub(crate) fn hue(s: &str) -> Hsla {
    const HUES: [&str; 7] = [
        "#1884fc", "#32bb78", "#f2994a", "#eb5757", "#9b51e0", "#2d9cdb", "#f2c94c",
    ];
    let i: usize = s.chars().map(|c| c as usize).sum::<usize>() % HUES.len();
    Hsla::from_str(HUES[i]).unwrap_or(theme::ACCENT.dark)
}

/// `plugTile` — the connector tile: the brand logo when the icon set has
/// the brand's paths, else the hue letter tile (App.tsx:1460).
pub(crate) fn plug_tile(name: &str, size: f32) -> impl IntoElement {
    let url = store().plugin_icon(name);
    if !url.is_empty() {
        return logo_tile(url, size, size * 0.25).into_element();
    }
    if crate::icons::has_brand(name) {
        return crate::icons::brand_tile(name, size).into_element();
    }
    let initial = name
        .chars()
        .next()
        .map(|c| c.to_ascii_uppercase().to_string())
        .unwrap_or_default();
    view()
        .size(px(size))
        .rounded(px(size * 0.25))
        .bg(hue(name))
        .border(px(0.5))
        .border_color(tc(0.06))
        .items_center()
        .justify_center()
        .child(
            text(initial)
                .text_size(px((size * 0.42).round()))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(Hsla::from_str("#ffffff").unwrap_or(theme::ACCENT.dark)),
        )
        .into_element()
}

/// A catalog logo (served by the app server) on a white plate, clipped
/// to the tile's radius — Grok Bot's plugin tiles.
/// Grok Bot's plugin tile: a white rounded plate in both themes (dark
/// marks like Vercel's stay visible), a 0.5pt `#1c1c1c14` edge, the
/// square logo inset 12%.
fn logo_tile(url: String, size: f32, _radius: f32) -> impl IntoElement {
    let inner = (size * 0.76).round();
    view()
        .size(px(size))
        .rounded(px((size * 0.28).round()))
        .overflow_hidden()
        .bg(white())
        .border(px(0.5))
        .border_color(Hsla::from_str("#1c1c1c14").unwrap_or(theme::ACCENT.dark))
        .items_center()
        .justify_center()
        .child(image(url).w(px(inner)).h(px(inner)))
}

/// `chipTile` — the overlapped logo in the "N connected" chip
/// (App.tsx:1468).
fn chip_tile(name: &str, size: f32) -> impl IntoElement {
    let url = store().plugin_icon(name);
    if !url.is_empty() {
        return logo_tile(url, size, size * 0.3).into_element();
    }
    if crate::icons::has_brand(name) {
        return crate::icons::brand_chip(name, size).into_element();
    }
    let initial = name
        .chars()
        .next()
        .map(|c| c.to_ascii_uppercase().to_string())
        .unwrap_or_default();
    view()
        .size(px(size))
        .rounded(px(size * 0.3))
        .bg(hue(name))
        .border(px(1.5))
        .border_color(tc(0.12))
        .items_center()
        .justify_center()
        .child(
            text(initial)
                .text_size(px((size * 0.45).round()))
                .font_weight(FontWeight::BOLD)
                .text_color(Hsla::from_str("#ffffff").unwrap_or(theme::ACCENT.dark)),
        )
        .into_element()
}

/// The overlay layer — mounted on the root stack.
pub(crate) fn layer() -> impl IntoElement {
    dynamic(|| {
        if !store().plugins_open.get() {
            return view().into_element();
        }
        modal().into_element()
    })
}

fn modal() -> impl IntoElement {
    view()
        .absolute()
        .size_full()
        .z_index(30)
        .items_center()
        .justify_center()
        // scrim — press closes (App.tsx:3491).
        .child(
            view()
                .absolute()
                .size_full()
                .bg(theme::tone(
                    theme::hsla(0.0, 0.0, 0.0, 0.35),
                    theme::hsla(0.0, 0.0, 0.0, 0.7),
                ))
                .on_press(|| store().close_plugins()),
        )
        .child(
            view()
                .w(percentage(1.0))
                .max_w(px(800.0))
                .mx(px(30.0))
                .h(percentage(1.0))
                .max_h(px(702.0))
                .my(px(30.0))
                .rounded(px(14.0))
                .bg(theme::tone(
                    theme::hsla(0.0, 0.0, 1.0, 1.0),
                    theme::hsla(0.0, 0.0, 0.122, 1.0),
                ))
                .border_1()
                .border_color(tc(0.10))
                .overflow_hidden()
                // the close button sits over every page at the market
                // header's position (App.tsx:3506).
                .child(
                    view()
                        .absolute()
                        .top(move || {
                            if store().plugin_page() == "market" {
                                px((MK_BAND - 34.0) / 2.0)
                            } else {
                                px(10.0)
                            }
                        })
                        .right(move || {
                            if store().plugin_page() == "market" {
                                px(MK_HEAD - 8.0 - 34.0)
                            } else {
                                px(10.0)
                            }
                        })
                        .size(move || {
                            if store().plugin_page() == "market" {
                                px(34.0)
                            } else {
                                px(28.0)
                            }
                        })
                        .rounded(px(17.0))
                        .z_index(4)
                        .items_center()
                        .justify_center()
                        .label("Close plugins")
                        .bg(move || {
                            if store().hover_x.get() {
                                tc(0.04)
                            } else {
                                AdaptiveColor::TRANSPARENT
                            }
                        })
                        .on_mouse_enter(|_: &MouseEvent| store().hover_x.set(true))
                        .on_mouse_leave(|_: &MouseEvent| store().hover_x.set(false))
                        .on_press(|| store().close_plugins())
                        .child(dynamic(|| {
                            if store().plugin_page() == "market" {
                                icon("close")
                            } else {
                                crate::icons::icon_ink("gb-close", 16.0, || theme::DIM)
                            }
                        })),
                )
                .child(
                    when(|| store().plugin_page() == "market", market_page)
                        .otherwise(|| {
                            when(|| store().plugin_page() == "connected", connected_page)
                                .otherwise(|| {
                                    when(|| store().plugin_page() == "plugin", plugin_page)
                                        .otherwise(|| {
                                            when(|| store().plugin_page() == "mcp", mcp_page)
                                                .otherwise(|| {
                                                    when(|| store().plugin_page() == "setup", setup_page)
                                                        .otherwise(|| {
                                                            when(|| store().plugin_page() == "v1plugin", v1_plugin_page)
                                                                .otherwise(|| {
                                                                    when(|| store().plugin_page() == "v1install", v1_install_page)
                                                                        .otherwise(skill_page)
                                                                })
                                                        })
                                                })
                                        })
                                })
                        })
                        .into_element(),
                ),
        )
}

/// The 48-tall detail bar shared by the inner pages (Grok Bot's
/// `sand-settings-detail-bar`: 10pt padding, a 28pt round back button —
/// or the "‹ Connect Apps" pill on the manager — and the page title
/// centered in 14pt/500).
fn back_bar(chev_title: Option<&'static str>, center: &str) -> impl IntoElement {
    let center = center.to_owned();
    let hov = signal(false);
    let back = view()
        .h(px(28.0))
        .rounded(px(999.0))
        .flex_row()
        .items_center()
        .label("Back")
        .bg(move || {
            if hov.get() || chev_title.is_some() {
                if hov.get() { tc(0.10) } else { tc(0.06) }
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(|| store().pop_nav());
    let back = match chev_title {
        Some(t) => back
            .pl(px(6.0))
            .pr(px(10.0))
            .gap(px(2.0))
            .child(view().size(px(20.0)).items_center().justify_center().child(
                crate::icons::icon_ink("gb-back", 16.0, || tc(0.68)),
            ))
            .child(
                text(t)
                    .text_size(px(13.0))
                    .line_height(px(18.0))
                    .text_color(tc(0.68)),
            ),
        None => back.w(px(28.0)).justify_center().child(crate::icons::icon_ink(
            "gb-back",
            16.0,
            || theme::DIM,
        )),
    };
    view()
        .h(px(48.0))
        .p(px(10.0))
        .child(
            view()
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .left(px(140.0))
                .right(px(140.0))
                .items_center()
                .justify_center()
                .child(
                    text(center)
                        .number_of_lines(1)
                        .text_size(px(14.0))
                        .line_height(px(20.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::TEXT),
                ),
        )
        .child(view().flex_row().child(back))
}

// ═══ MARKET page (App.tsx:3522) ═══

fn market_page() -> impl IntoElement {
    // A new category starts at the top (the virtual list would otherwise
    // keep the old scroll offset).
    let body = element_ref();
    let b2 = body.clone();
    effect(move || {
        let _ = store().plugin_cat.get();
        let _ = store().plugin_query.get();
        store().plugin_limit.set(60);
        b2.scroll_to_offset(px(0.0), false);
    });
    view()
        .flex_1()
        // fixed header — search row + Bot templates; the hairline is
        // pinned to the band's bottom edge.
        .child(
            view()
                .h(px(MK_BAND))
                .pt(px((MK_BAND - 36.0) / 2.0))
                .pl(px(MK_LEFT))
                .pr(px(MK_HEAD))
                .z_index(3)
                .child(search_row())
                .child(
                    view()
                        .absolute()
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .h(px(1.0))
                        .bg(tc(0.09)),
                ),
        )
        .child(
            scroll().flex_1().element_ref(&body).child(
                view()
                    .pl(px(MK_LEFT))
                    .pr(px(MK_RIGHT))
                    .pb(px(24.0))
                    // first scroll row — Connect Apps + connected chip.
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .h(px(MK_TITLE))
                            .child(
                                text("Connect Apps")
                                    .text_size(px(17.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::TEXT),
                            )
                            .child(view().flex_1())
                            .child(conn_chip()),
                    )
                    // category pills scroll with the content.
                    .child(
                        view()
                            .flex_row()
                            .gap(px(8.0))
                            .flex_wrap()
                            .py(px(12.0))
                            .child(each(
                                || visible_cats(),
                                |c| c.clone(),
                                |c, _| cat_pill(c.get()),
                            ))
                            .child(when(
                                || store().plugin_cats().len() > CAT_CAP + 1,
                                more_cats_btn,
                            )),
                    )
                    .child(when(
                        || {
                            !store().plugin_query.get().is_empty()
                                && store().plugin_list().is_empty()
                        },
                        || {
                            text("No plugins match")
                                .text_size(px(13.0))
                                .text_color(theme::DIM)
                                .py(px(24.0))
                                .self_center()
                        },
                    ))
                    // Home ("All", no query): Recommended + six per
                    // category with View all, like Grok Bot's browse page.
                    .child(when(
                        || store().plugin_query.get().is_empty() && store().plugin_cat.get() == "All",
                        || {
                            view()
                                .child(when(
                                    || !store().plugin_recommended().is_empty(),
                                    || {
                                        view()
                                            .child(section_head(crate::store::RECOMMENDED, crate::store::RECOMMENDED, 10.0))
                                            .child(grid2(|| store().plugin_recommended().into_iter().take(6).collect()))
                                    },
                                ))
                                .child(each(
                                    || store().plugin_cats().into_iter().filter(|c| c != "All").collect::<Vec<_>>(),
                                    |c| c.clone(),
                                    |c, _| cat_section(c.get()),
                                ))
                        },
                    ))
                    // A category (or a search): its full list, virtualized
                    // inside the page's scroll.
                    .child(when(
                        || {
                            (!store().plugin_query.get().is_empty() || store().plugin_cat.get() != "All")
                                && !store().plugin_list().is_empty()
                        },
                        full_list,
                    )),
            ),
        )
        // search combo box — same left edge and width as the field.
        .child(when(|| store().drop_open(), drop_layer))
}

fn search_row() -> impl IntoElement {
    let field = element_ref();
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .h(px(36.0))
        .child(
            view()
                .element_ref(&field)
                .flex_1()
                .h(px(36.0))
                .rounded(px(18.0))
                .border(px(0.5))
                .border_color(tc(0.09))
                .bg(tc(0.05))
                .flex_row()
                .items_center()
                .pl(px(13.0))
                .pr(px(8.0))
                .gap(px(7.0))
                .on_layout(move |b: &Bounds<Points>| {
                    store().field_w.set(b.size.width.0)
                })
                .child(icon("search"))
                .child(
                    input()
                        .flex_1()
                        .value(move || store().plugin_query.get())
                        .placeholder("Search across apps and skills")
                        .placeholder_color(|| color(theme::GHOST))
                        .text_size(px(14.0))
                        .text_color(theme::TEXT)
                        .on_change(|t: &str| {
                            let st = store();
                            st.plugin_query.set(t.to_owned());
                            // `effect` (App.tsx:1541): a query change resets
                            // hidden + selection.
                            st.drop_hidden.set(false);
                            st.drop_sel.set(0);
                        })
                        .on_submit(|_: &str| store().drop_choose()),
                )
                .child(when(
                    || !store().plugin_query.get().is_empty(),
                    || {
                        text("Clear")
                            .text_size(px(12.0))
                            .text_color(theme::DIM)
                            .px_1()
                            .on_press(|| store().plugin_query.set(String::new()))
                    },
                )),
        )
        // Bot templates pill (App.tsx:3579) — display-only in Grok too.
        .child(
            view()
                .h(px(36.0))
                .rounded(px(18.0))
                .border(px(0.5))
                .border_color(tc(0.09))
                .pl(px(9.0))
                .pr(px(13.0))
                .flex_row()
                .items_center()
                .gap(px(7.0))
                .bg(move || {
                    if store().hover_tpl.get() {
                        tc(0.04)
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .on_mouse_enter(|_: &MouseEvent| store().hover_tpl.set(true))
                .on_mouse_leave(|_: &MouseEvent| store().hover_tpl.set(false))
                // The app's default mark: black on light, white on dark.
                .child(dynamic(|| {
                    crate::avatar::bot_blob(crate::avatar::BotBlobProps {
                        shape: "blob".into(),
                        color: "#FFFFFF".into(),
                        size: 20.0,
                        paused: true,
                        ..Default::default()
                    })
                    .into_element()
                }))
                .child(
                    text("Bot templates")
                        .text_size(px(13.5))
                        .text_color(theme::TEXT),
                )
                .child(icon("arrowUpRight")),
        )
}

fn conn_chip() -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .h(px(28.0))
        .pl_1()
        .pr_2()
        .rounded(px(8.0))
        .bg(move || {
            if store().hover_chip.get() {
                tc(0.04)
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .on_mouse_enter(|_: &MouseEvent| store().hover_chip.set(true))
        .on_mouse_leave(|_: &MouseEvent| store().hover_chip.set(false))
        .on_press(|| store().push_nav("connected", "", ""))
        .child(
            view()
                .flex_row()
                .pl(px(2.0))
                .child(each(
                    || store().connected_names().into_iter().take(4).collect::<Vec<_>>(),
                    |n| n.clone(),
                    |n, i| {
                        let neg = if i.get() == 0 { 0.0 } else { -7.0 };
                        view().ml(px(neg)).child(chip_tile(&n.get(), 18.0))
                    },
                )),
        )
        .child(
            text(move || {
                let st = store();
                format!("{} Connected / {} Skills", st.connected_names().len(), st.private_skills.get().len())
            })
            .text_size(px(13.0))
            .text_color(theme::DIM),
        )
        .child(icon("chevronSmRight"))
}

fn visible_cats() -> Vec<String> {
    let all = store().plugin_cats();
    if all.len() <= CAT_CAP + 1 {
        return all;
    }
    let head: Vec<String> = all.iter().take(CAT_CAP).cloned().collect();
    let sel = store().plugin_cat.get();
    if head.iter().any(|c| c == &sel) {
        head
    } else {
        head.into_iter().chain([sel]).collect()
    }
}

fn more_cats_btn() -> impl IntoElement {
    let node = element_ref();
    view()
        .element_ref(&node)
        .h(px(26.0))
        .rounded(px(13.0))
        .px_3()
        .flex_row()
        .items_center()
        .gap_1()
        .bg(tc(0.06))
        .on_press(move || {
            spawn(async move {
                let m = node.measure().await;
                let (x, y) = m
                    .map(|m| (m.page.x.0, m.page.y.0 + m.frame.size.height.0 + 6.0))
                    .unwrap_or((0.0, 0.0));
                let sel = store().plugin_cat.get();
                let items: Vec<crate::store::CtxItem> = store()
                    .plugin_cats()
                    .into_iter()
                    .skip(CAT_CAP)
                    .map(|c| {
                        let mut it = crate::store::CtxItem::row(
                            &c,
                            "",
                            "plugin-cat",
                            c.clone(),
                        );
                        it.checked = sel == c;
                        it
                    })
                    .collect();
                crate::ctx::open_at(x, y, items);
            });
        })
        .child(
            text("More")
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::TEXT),
        )
        .child(icon("chevronDown"))
}

fn cat_pill(c: String) -> impl IntoElement {
    let c2 = c.clone();
    let c3 = c.clone();
    let c4 = c.clone();
    view()
        .h(px(26.0))
        .rounded(px(13.0))
        .px_3()
        .flex_row()
        .items_center()
        .gap(px(3.0))
        .bg(move || {
            if store().plugin_cat.get() == c2 {
                tc(0.96)
            } else {
                tc(0.06)
            }
        })
        .on_press(move || store().plugin_cat.set(c.clone()))
        .child(
            text(c3)
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                // The selected pill is filled with the ink, so its text is
                // the page: white on light, near-black on dark.
                .text_color(move || {
                    if store().plugin_cat.get() == c4 {
                        theme::tone(theme::hsla(0.0, 0.0, 1.0, 1.0), theme::hsla(0.0, 0.0, 0.071, 1.0))
                    } else {
                        theme::TEXT
                    }
                }),
        )
}

fn section_head(label: &'static str, cat: &'static str, top: f32) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .justify_between()
        .pt(px(top))
        .pb_1()
        .child(
            text(label)
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::TEXT),
        )
        .child(
            view().h(px(20.0)).justify_center().on_press(move || {
                store().plugin_cat.set(cat.to_owned())
            }).child(
                text("View all").text_size(px(12.0)).text_color(theme::DIM),
            ),
        )
}

fn cat_section(cat: String) -> impl IntoElement {
    let label = cat.clone();
    view()
        .child(section_head_owned(&label, 22.0))
        .child(grid2(move || store().plugins_in(&cat).into_iter().take(6).collect()))
}

/// Every plugin of the selected category / search, two per row — 60 at a
/// time ("Show more" adds the next 60) so a long list stays cheap.
fn full_list() -> impl IntoElement {
    view()
        .child(
            text(|| {
                let st = store();
                if !st.plugin_query.get().is_empty() {
                    "Results".to_owned()
                } else {
                    st.plugin_cat.get()
                }
            })
            .text_size(px(13.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::TEXT)
            .pt(px(10.0))
            .pb_1(),
        )
        .child(grid2(|| {
            let n = store().plugin_limit.get();
            store().plugin_list().into_iter().take(n).collect()
        }))
        .child(when(
            || store().plugin_list().len() > store().plugin_limit.get(),
            || {
                let hov = signal(false);
                view().items_center().pt(px(10.0)).child(
                    view()
                        .h(px(30.0))
                        .px(px(16.0))
                        .rounded(px(15.0))
                        .justify_center()
                        .bg(move || if hov.get() { tc(0.10) } else { tc(0.06) })
                        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
                        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
                        .on_press(|| store().plugin_limit.update(|n| *n += 60))
                        .child(
                            text(|| {
                                let left = store().plugin_list().len().saturating_sub(store().plugin_limit.get());
                                format!("Show more ({left})")
                            })
                            .text_size(px(13.0))
                            .text_color(theme::TEXT),
                        ),
                )
            },
        ))
}

fn section_head_owned(label: &str, top: f32) -> impl IntoElement {
    let cat = label.to_owned();
    view()
        .flex_row()
        .items_center()
        .justify_between()
        .pt(px(top))
        .pb_1()
        .child(
            text(label.to_owned())
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::TEXT),
        )
        .child(
            view().h(px(20.0)).justify_center().on_press(move || {
                store().plugin_cat.set(cat.clone())
            }).child(
                text("View all").text_size(px(12.0)).text_color(theme::DIM),
            ),
        )
}

/// `pluginPairs` → rows of two (App.tsx:1477); a pair of one leaves a
/// flex-1 hole so singles stay half width.
fn grid2(items: impl Fn() -> Vec<PluginRow> + 'static) -> impl IntoElement {
    view().gap(px(6.0)).child(each(
        move || {
            let items = items();
            let mut out: Vec<Vec<PluginRow>> = Vec::new();
            for i in (0..items.len()).step_by(2) {
                out.push(items[i..(i + 2).min(items.len())].to_vec());
            }
            out
        },
        |pair| {
            pair.iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join("|")
        },
        |pair, _| {
            let pair = pair.get();
            let single = pair.len() == 1;
            view()
                .flex_row()
                .gap(px(8.0))
                .child(each(
                    move || pair.clone(),
                    |p| p.name.clone(),
                    |p, _| plugin_row(p.get()),
                ))
                .when(single, |v| v.child(view().flex_1()))
        },
    ))
}

/// `pluginRow` — 72 tall, 38px tile, Add pill on the right edge
/// (App.tsx:1489).
fn plugin_row(p: PluginRow) -> impl IntoElement {
    let name = p.name.clone();
    let n_hin = p.name.clone();
    let n_hout = p.name.clone();
    let n_press = p.name.clone();
    let name4 = p.name.clone();
    let n_ain = p.name.clone();
    let n_aout = p.name.clone();
    let n_inst = p.name.clone();
    let n_busy = p.name.clone();
    view()
        .flex_1()
        .min_w(px(0.0))
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .h(px(72.0))
        .rounded(px(14.0))
        .px_3()
        .bg(move || {
            if store().hover_plugin.get() == name {
                tc(0.04)
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .on_mouse_enter(move |_: &MouseEvent| store().hover_plugin.set(n_hin.clone()))
        .on_mouse_leave(move |_: &MouseEvent| {
            if store().hover_plugin.get_untracked() == n_hout {
                store().hover_plugin.set(String::new())
            }
        })
        .on_press(move || {
            let n = n_press.clone();
            store().push_nav("plugin", &n, "")
        })
        .child(plug_tile(&p.name, 38.0))
        .child(
            view()
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .max_w(px(200.0))
                .child(
                    text(if p.display_name.is_empty() { Store::plugin_title(&p.name) } else { p.display_name.clone() })
                        .number_of_lines(1)
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::TEXT),
                )
                .child(
                    text(p.description.clone())
                        .number_of_lines(1)
                        .text_size(px(12.0))
                        .text_color(theme::DIM),
                ),
        )
        .child(view().flex_1())
        .child(if p.installed {
            text("✓ Added")
                .text_size(px(12.0))
                .text_color(theme::DIM)
                .into_element()
        } else {
            view()
                .h(px(30.0))
                .min_w(px(56.0))
                .px(px(22.0))
                .rounded(px(15.0))
                .items_center()
                .justify_center()
                .bg(move || {
                    if store().hover_add.get() == name4 {
                        tc(0.17)
                    } else if store().hover_plugin.get() == name4 {
                        tc(0.10)
                    } else {
                        tc(0.06)
                    }
                })
                .on_mouse_enter(move |_: &MouseEvent| store().hover_add.set(n_ain.clone()))
                .on_mouse_leave(move |_: &MouseEvent| {
                    if store().hover_add.get_untracked() == n_aout {
                        store().hover_add.set(String::new())
                    }
                })
                .on_press(move || store().install_plugin(&n_inst))
                .child(
                    text(move || {
                        if store().plugin_busy.get() == n_busy {
                            "Adding…".to_owned()
                        } else {
                            "Add".to_owned()
                        }
                    })
                    .text_size(px(13.0))
                    .text_color(theme::TEXT),
                )
                .into_element()
        })
}

/// The search combo box (App.tsx:3707).
fn drop_layer() -> impl IntoElement {
    view()
        .absolute()
        .left(px(MK_LEFT))
        .w(move || px(store().field_w.get()))
        .top(px((MK_BAND + 36.0) / 2.0 + 4.0))
        .h(move || {
            let n = store().plugin_list().len() as f32;
            px(DROP_MAX.min(DROP_HEAD + n * DROP_ROW + 8.0))
        })
        .z_index(5)
        .rounded(px(12.0))
        .bg(theme::tone(
            theme::hsla(0.0, 0.0, 1.0, 1.0),
            theme::hsla(0.0, 0.0, 0.149, 1.0),
        ))
        .border_1()
        .border_color(tc(0.12))
        .overflow_hidden()
        .child(
            text("Plugins")
                .text_size(px(12.0))
                .text_color(theme::DIM)
                .h(px(DROP_HEAD))
                .pl(px(14.0))
                .pt(px(10.0)),
        )
        .child(
            scroll().flex_1().child(
                view().pb_2().child(each(
                    || store().plugin_list(),
                    |p| p.name.clone(),
                    |p, i| {
                        let name = p.get().name.clone();
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(10.0))
                            .px_3()
                            .h(px(DROP_ROW))
                            .bg(move || {
                                if store().drop_sel.get() == i.get() {
                                    tc(0.06)
                                } else {
                                    AdaptiveColor::TRANSPARENT
                                }
                            })
                            .on_mouse_enter(move |_: &MouseEvent| store().drop_sel.set(i.get()))
                            .on_press(move || {
                                let n = name.clone();
                                store().push_nav("plugin", &n, "")
                            })
                            .child(
                                view()
                                    .absolute()
                                    .left(px(0.0))
                                    .top(px(6.0))
                                    .bottom(px(6.0))
                                    .w(px(2.0))
                                    .rounded(px(1.0))
                                    .bg(move || {
                                        if store().drop_sel.get() == i.get() {
                                            tc(0.54)
                                        } else {
                                            AdaptiveColor::TRANSPARENT
                                        }
                                    }),
                            )
                            .child(plug_tile(&p.get().name, 24.0))
                            .child(
                                text(store().plugin_display(&p.get().name))
                                    .number_of_lines(1)
                                    .flex_1()
                                    .text_size(px(13.0))
                                    .text_color(theme::TEXT),
                            )
                            .child(icon("chevronSmRight"))
                    },
                )),
            ),
        )
}

// ═══ CONNECTED page — "Manage plugins and skills" (App.tsx:3754) ═══

#[derive(Clone, PartialEq)]
enum ConnItem {
    Plugin(PluginRow),
    Mcp(McpServer),
}

fn connected_page() -> impl IntoElement {
    // Fills the card: its scroll is `flex_1`, with no height of its
    // own the page drew nothing.
    view()
        .flex_1()
        .child(back_bar(Some("Connect Apps"), ""))
        .child(
            scroll().flex_1().child(
                view()
                    .px(px(21.0))
                    .pb(px(24.0))
                    .child(
                        text("Manage plugins and skills")
                            .text_size(px(17.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT)
                            .pt_1()
                            .pb(px(14.0)),
                    )
                    .child(v1_section())
                    .child(
                        text("Installed")
                            .text_size(px(12.0))
                            .text_color(theme::DIM)
                            .pl(px(2.0))
                            .pb_2(),
                    )
                    .child(each(
                        || {
                            let st = store();
                            let (inst, standalone) = st.conn_items();
                            let shown = if st.show_all_installed.get() {
                                inst
                            } else {
                                inst.into_iter().take(4).collect()
                            };
                            let mut items: Vec<ConnItem> = Vec::new();
                            for p in shown {
                                items.push(ConnItem::Plugin(p));
                            }
                            for s in standalone {
                                items.push(ConnItem::Mcp(s));
                            }
                            let mut out: Vec<Vec<ConnItem>> = Vec::new();
                            for i in (0..items.len()).step_by(2) {
                                out.push(items[i..(i + 2).min(items.len())].to_vec());
                            }
                            out
                        },
                        |pair| {
                            pair.iter()
                                .map(|i| match i {
                                    ConnItem::Plugin(p) => p.name.as_str(),
                                    ConnItem::Mcp(s) => s.id.as_str(),
                                })
                                .collect::<Vec<_>>()
                                .join("|")
                        },
                        |pair, _| {
                            let pair = pair.get();
                            let single = pair.len() == 1;
                            view()
                                .flex_row()
                                .gap(px(8.0))
                                .pb_2()
                                .child(each(
                                    move || pair.clone(),
                                    |i| match i {
                                        ConnItem::Plugin(p) => p.name.clone(),
                                        ConnItem::Mcp(s) => s.id.clone(),
                                    },
                                    |i, _| conn_tile(i.get()),
                                ))
                                .when(single, |v| v.child(view().flex_1()))
                        },
                    ))
                    .child(when(
                        || {
                            store()
                                .plugins
                                .get()
                                .iter()
                                .filter(|p| p.installed)
                                .count()
                                > 4
                        },
                        || {
                            text(move || {
                                if store().show_all_installed.get() {
                                    "Show less".to_owned()
                                } else {
                                    format!(
                                        "Show all {} plugins",
                                        store()
                                            .plugins
                                            .get()
                                            .iter()
                                            .filter(|p| p.installed)
                                            .count()
                                    )
                                }
                            })
                            .text_size(px(12.0))
                            .text_color(theme::DIM)
                            .py(px(10.0))
                            .on_press(|| {
                                store().show_all_installed.update(|v| *v = !*v)
                            })
                        },
                    ))
                    // Private skills (App.tsx:3826).
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .pt(px(18.0))
                            .pb_2()
                            .pr_1()
                            .child(
                                text("Private skills")
                                    .text_size(px(12.0))
                                    .text_color(theme::DIM)
                                    .pl(px(2.0)),
                            )
                            .child(
                                view().px(px(6.0)).on_press(|| {
                                    store().push_nav("skill", "", "")
                                }).child(icon("plus")),
                            ),
                    )
                    .child(when(
                        || store().private_skills.get().is_empty(),
                        || {
                            text("None yet — ask the agent to save one (\"remember to always…\") or tap ＋.")
                                .text_size(px(12.0))
                                .text_color(theme::FAINT)
                                .pl(px(2.0))
                                .pb(px(6.0))
                        },
                    ))
                    .child(each(
                        || store().private_skills.get(),
                        |s| s.id.clone(),
                        |s, _| {
                            let sk = s.get();
                            let sk2 = sk.clone();
                            let id = sk.id.clone();
                            view()
                                .flex_row()
                                .gap(px(12.0))
                                .py(px(10.0))
                                .items_start()
                                .on_press(move || {
                                    let id = id.clone();
                                    store().push_nav("skill", "", &id)
                                })
                                .child(
                                    view()
                                        .size(px(36.0))
                                        .rounded(px(9.0))
                                        .bg(tc(0.07))
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            text("📦")
                                                .text_size(px(15.0))
                                                .text_color(theme::DIM),
                                        ),
                                )
                                .child(
                                    view()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .child(
                                            text(sk.name.clone())
                                                .number_of_lines(1)
                                                .text_size(px(13.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(if sk.enabled {
                                                    theme::TEXT
                                                } else {
                                                    tc(0.4)
                                                }),
                                        )
                                        .child(
                                            text(format!(
                                                "Created locally · {}",
                                                sk.description
                                            ))
                                            .number_of_lines(1)
                                            .text_size(px(12.0))
                                            .text_color(theme::DIM),
                                        ),
                                )
                                .when(!sk2.enabled, |v| {
                                    v.child(
                                        text("off")
                                            .text_size(px(11.0))
                                            .text_color(theme::DIM)
                                            .pt(px(3.0)),
                                    )
                                })
                        },
                    )),
            ),
        )
}

/// An installed-plugin or standalone-server tile (App.tsx:3778/3797).
fn conn_tile(item: ConnItem) -> impl IntoElement {
    match item {
        ConnItem::Plugin(p) => {
            let name = p.name.clone();
            let name2 = p.name.clone();
            let name3 = p.name.clone();
            let name4 = p.name.clone();
            let name5 = p.name.clone();
            let key = format!("p:{}", p.name);
            let key_in = key.clone();
            let key_out = key.clone();
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .px(px(14.0))
                .py(px(10.0))
                .rounded(px(12.0))
                .bg(move || {
                    if store().hover_inst.get() == key {
                        tc(0.06)
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .on_mouse_enter(move |_: &MouseEvent| store().hover_inst.set(key_in.clone()))
                .on_mouse_leave(move |_: &MouseEvent| {
                    if store().hover_inst.get_untracked() == key_out {
                        store().hover_inst.set(String::new())
                    }
                })
                .on_press(move || {
                    let n = name.clone();
                    store().push_nav("plugin", &n, "")
                })
                .child(plug_tile(&p.name, 36.0))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(
                            text(Store::plugin_title(&name2))
                                .number_of_lines(1)
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme::TEXT),
                        )
                        .child(
                            text(move || {
                                let st = store();
                                match st.plugin_details.get().get(&name3) {
                                    Some(d) => format!(
                                        "{} connector{}, {} skill{}",
                                        d.mcp.len(),
                                        if d.mcp.len() == 1 { "" } else { "s" },
                                        d.skills.len(),
                                        if d.skills.len() == 1 { "" } else { "s" },
                                    ),
                                    None => "…".to_owned(),
                                }
                            })
                            .number_of_lines(1)
                            .text_size(px(11.0))
                            .text_color(theme::DIM),
                        ),
                )
                .child(
                    text(move || {
                        let st = store();
                        match st.plugin_details.get().get(&name4) {
                            Some(d) if d.mcp.iter().any(|m| m.status == "connected") => {
                                "Connected".to_owned()
                            }
                            Some(d) if d.mcp.iter().any(|m| m.status == "error") => {
                                "Error".to_owned()
                            }
                            Some(_) => "Installed".to_owned(),
                            None => "…".to_owned(),
                        }
                    })
                    .text_size(px(12.0))
                    .text_color(move || {
                        let st = store();
                        match st.plugin_details.get().get(&name5) {
                            Some(d) if d.mcp.iter().any(|m| m.status == "connected") => {
                                hx("#32bb78")
                            }
                            Some(d) if d.mcp.iter().any(|m| m.status == "error") => hx("#ff5f57"),
                            _ => theme::DIM,
                        }
                    }),
                )
                .into_element()
        }
        ConnItem::Mcp(s) => {
            let id = s.id.clone();
            let key = format!("m:{}", s.id);
            let key_in = key.clone();
            let key_out = key.clone();
            let label = if s.name.is_empty() {
                s.id.clone()
            } else {
                s.name.clone()
            };
            let status_txt = match s.status.as_str() {
                "connected" => "Connected".to_owned(),
                "error" => "Error".to_owned(),
                _ => {
                    if s.enabled {
                        "Idle".to_owned()
                    } else {
                        "Off".to_owned()
                    }
                }
            };
            let status_col = match s.status.as_str() {
                "connected" => hx("#32bb78"),
                "error" => hx("#ff5f57"),
                _ => theme::DIM,
            };
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .px(px(14.0))
                .py(px(10.0))
                .rounded(px(12.0))
                .bg(move || {
                    if store().hover_inst.get() == key {
                        tc(0.06)
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .on_mouse_enter(move |_: &MouseEvent| store().hover_inst.set(key_in.clone()))
                .on_mouse_leave(move |_: &MouseEvent| {
                    if store().hover_inst.get_untracked() == key_out {
                        store().hover_inst.set(String::new())
                    }
                })
                .on_press(move || {
                    let id = id.clone();
                    store().push_nav("mcp", "", &id)
                })
                .child(plug_tile(&label, 36.0))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(
                            text(label.clone())
                                .number_of_lines(1)
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme::TEXT),
                        )
                        .child(
                            text("1 connector")
                                .number_of_lines(1)
                                .text_size(px(11.0))
                                .text_color(theme::DIM),
                        ),
                )
                .child(
                    text(status_txt)
                        .text_size(px(12.0))
                        .text_color(status_col),
                )
                .into_element()
        }
    }
}

// ═══ Detail pages (plugin / MCP server / private skill) ═══
//
// Grok Bot's `sand-plugins-detail` pages, measured off its live DOM: a
// 22/32/28 settings pane; 56pt header tile + 17pt/500 title; 12pt section
// labels (8/14/6 padding); white cards with a 0.5pt 8% hairline, radius
// 14, 8pt vertical padding and 10/14 rows; 13pt rows over 12pt captions.

/// `--sand-text-secondary` (#1c1c1cad) — descriptions, "View Source".
fn ink2() -> AdaptiveColor {
    tc(0.68)
}

/// `--sand-text-tertiary` (#1c1c1c8f) — labels, captions, row icons.
fn ink3() -> AdaptiveColor {
    tc(0.56)
}

fn pair(light: &str, dark: &str) -> AdaptiveColor {
    let d = Hsla::from_str(dark).unwrap_or(theme::ACCENT.dark);
    theme::tone(Hsla::from_str(light).unwrap_or(d), d)
}

/// `--sand-text-success-primary` / `-danger-` / `-warn-` (light from
/// Grok Bot; dark the Grok Shell palette's).
fn ok_ink() -> AdaptiveColor {
    pair("#00854a", "#32bb78")
}
fn danger_ink() -> AdaptiveColor {
    pair("#cd2834", "#ff6b64")
}
fn warn_ink() -> AdaptiveColor {
    pair("#aa6300", "#ffb224")
}

/// `--sand-bg-primary` + `--sand-text-inverted` — the filled button.
fn primary_bg() -> AdaptiveColor {
    pair("#1c1c1c", "#f5f5f5")
}
fn primary_ink() -> AdaptiveColor {
    pair("#fcfcfc", "#1c1c1c")
}

fn white() -> Hsla {
    Hsla::from_str("#ffffff").unwrap_or(theme::ACCENT.dark)
}

/// A detail card: white on light, a faint wash on dark; 0.5pt hairline.
fn card() -> View {
    view()
        .bg(theme::tone(theme::hsla(0.0, 0.0, 1.0, 1.0), theme::hsla(0.0, 0.0, 0.96, 0.03)))
        .border(px(0.5))
        .border_color(tc(0.08))
        .rounded(px(14.0))
        .overflow_hidden()
}

/// `sectionLabel` — 12pt tertiary, 8/14/6 padding (30 tall).
fn sec_label(label: impl Into<String>) -> impl IntoElement {
    text(label.into())
        .text_size(px(12.0))
        .line_height(px(16.0))
        .text_color(ink3())
        .pt(px(8.0))
        .pb(px(6.0))
        .px(px(14.0))
}

/// "Apps 1" / "Skills 33" — the label in primary ink, the count after.
fn count_label(label: &'static str, count: impl Fn() -> usize + 'static) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .pt(px(8.0))
        .pb(px(6.0))
        .px(px(14.0))
        .child(text(label).text_size(px(12.0)).line_height(px(16.0)).text_color(theme::TEXT))
        .child(
            text(move || count().to_string())
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_color(ink3()),
        )
}

/// A 26-tall small button (`size: sm`): secondary = 6% fill, danger =
/// light-red fill + red ink, primary = filled, ghost = bare.
#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Secondary,
    Danger,
    DangerSolid,
    Primary,
    Ghost,
}

fn sm_btn(
    kind: Btn,
    label: impl Fn() -> String + 'static,
    on: impl Fn() + 'static,
) -> impl IntoElement {
    let hov = signal(false);
    let (bg, bg_hov, ink) = match kind {
        Btn::Secondary => (tc(0.06), tc(0.10), theme::TEXT),
        Btn::Danger => (
            pair("#fb2b3e17", "#ff5f5726"),
            pair("#fb2b3e2b", "#ff5f573d"),
            danger_ink(),
        ),
        Btn::DangerSolid => (
            pair("#e72e3b", "#e72e3b"),
            pair("#c82532", "#c82532"),
            AdaptiveColor::from(white()),
        ),
        Btn::Primary => (primary_bg(), primary_bg(), primary_ink()),
        Btn::Ghost => (AdaptiveColor::TRANSPARENT, tc(0.06), theme::TEXT),
    };
    view()
        .h(px(26.0))
        .rounded(px(13.0))
        .px(px(10.0))
        .items_center()
        .justify_center()
        .bg(move || if hov.get() { bg_hov } else { bg })
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(move || on())
        .child(
            text(label)
                .number_of_lines(1)
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_color(ink),
        )
}

/// The header's 36-tall pills (Share / Uninstall / Add): 6% fill, 14pt.
fn pill_btn(
    glyph: Option<&'static str>,
    label: impl Fn() -> String + 'static,
    on: impl Fn() + 'static,
) -> impl IntoElement {
    let hov = signal(false);
    view()
        .h(px(36.0))
        .rounded(px(999.0))
        .px(px(16.0))
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(4.0))
        .bg(move || if hov.get() { tc(0.10) } else { tc(0.06) })
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(move || on())
        .when(glyph.is_some(), |v| {
            v.child(
                view()
                    .size(px(18.0))
                    .items_center()
                    .justify_center()
                    .child(icon_ink(glyph.unwrap_or_default(), 14.0, || theme::TEXT)),
            )
        })
        .child(
            text(label)
                .text_size(px(14.0))
                .line_height(px(20.0))
                .text_color(theme::TEXT),
        )
}

fn icon_ink(name: &str, size: f32, ink: impl Fn() -> AdaptiveColor + 'static) -> AnyElement {
    crate::icons::icon_ink(name, size, ink)
}

/// A 13pt row with a 16pt glyph and a 12pt caption (Apps / Skills).
fn glyph_row(glyph: &'static str, title: String, caption: String) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(14.0))
        .py(px(10.0))
        .child(icon_ink(glyph, 16.0, ink3))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(
                    text(title)
                        .number_of_lines(1)
                        .text_size(px(13.0))
                        .line_height(px(18.0))
                        .text_color(theme::TEXT),
                )
                .child(
                    text(caption)
                        .number_of_lines(1)
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .text_color(ink3()),
                ),
        )
}

/// `dl` row — 13pt term, 13pt tertiary value right-aligned, 38 tall.
fn info_row(label: &'static str, value: impl Fn() -> String + 'static) -> impl IntoElement {
    view()
        .flex_row()
        .items_start()
        .justify_between()
        .gap(px(20.0))
        .px(px(14.0))
        .py(px(10.0))
        .child(
            text(label)
                .text_size(px(13.0))
                .line_height(px(18.0))
                .text_color(theme::TEXT),
        )
        .child(
            text(value)
                .number_of_lines(2)
                .flex_shrink(1.0)
                .text_size(px(13.0))
                .line_height(px(18.0))
                .text_color(ink3()),
        )
}

/// The detail pages' scroll body — the 22/32/28 settings pane.
fn pane() -> View {
    view().pt(px(22.0)).px(px(32.0)).pb(px(28.0))
}

/// The status word of an account / server row.
fn status_text(label: &str, ink: AdaptiveColor) -> AnyElement {
    text(label.to_owned())
        .text_size(px(13.0))
        .line_height(px(18.0))
        .text_color(ink)
        .into_element()
}

// ═══ PLUGIN detail page ═══

fn plugin_page() -> impl IntoElement {
    let name = store()
        .plugin_nav
        .get_untracked()
        .last()
        .map(|n| n.name.clone())
        .unwrap_or_default();
    let title = store().plugin_display(&name);
    let n_desc = name.clone();
    let n_acc = name.clone();
    let n_setup = name.clone();
    let n_setup2 = name.clone();
    let n_tools = name.clone();
    let n_apps = name.clone();
    let n_skills = name.clone();
    let n_src = name.clone();
    let n_copy = name.clone();
    let n_share = name.clone();
    view()
        .flex_1()
        .child(back_bar(None, &title))
        .child(
            scroll().flex_1().child(
                pane()
                    // header: tile + name/copy-link + View Source ↗ ··· Share, Uninstall
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .h(px(56.0))
                            .child(
                                view()
                                    .size(px(56.0))
                                    .rounded(px(16.0))
                                    .overflow_hidden()
                                    .border(px(0.5))
                                    .border_color(tc(0.08))
                                    .child(plug_tile(&name, 56.0)),
                            )
                            .child(
                                view()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .ml(px(12.0))
                                    .gap(px(2.0))
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(4.0))
                                            .child(
                                                text(title.clone())
                                                    .number_of_lines(1)
                                                    .text_size(px(17.0))
                                                    .line_height(px(24.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(theme::TEXT),
                                            )
                                            .child({
                                                let hov = signal(false);
                                                view()
                                                    .size(px(20.0))
                                                    .rounded(px(6.0))
                                                    .items_center()
                                                    .justify_center()
                                                    .label("Copy link to this plugin")
                                                    .bg(move || {
                                                        if hov.get() {
                                                            tc(0.06)
                                                        } else {
                                                            AdaptiveColor::TRANSPARENT
                                                        }
                                                    })
                                                    .on_mouse_enter(move |_: &MouseEvent| {
                                                        hov.set(true)
                                                    })
                                                    .on_mouse_leave(move |_: &MouseEvent| {
                                                        hov.set(false)
                                                    })
                                                    .on_press(move || store().copy_plugin_link(&n_copy))
                                                    .child(icon_ink("gb-link", 12.0, || theme::DIM))
                                            }),
                                    )
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(4.0))
                                            .on_press(move || {
                                                let d = store()
                                                    .plugin_details
                                                    .get_untracked()
                                                    .get(&n_src)
                                                    .cloned()
                                                    .unwrap_or_default();
                                                let url = if d.source.is_empty() { d.homepage } else { d.source };
                                                if !url.is_empty() {
                                                    store().open_link(&url);
                                                }
                                            })
                                            .child(
                                                text("View Source")
                                                    .text_size(px(13.0))
                                                    .line_height(px(18.0))
                                                    .text_color(ink2()),
                                            )
                                            .child(icon_ink("gb-arrow-up-right", 13.0, ink2)),
                                    ),
                            )
                            .child(
                                view()
                                    .flex_row()
                                    .gap(px(8.0))
                                    .ml(px(12.0))
                                    .child(pill_btn(
                                        Some("gb-link"),
                                        || {
                                            if store().plugin_copied.get() {
                                                "Copied".to_owned()
                                            } else {
                                                "Share".to_owned()
                                            }
                                        },
                                        move || store().copy_plugin_link(&n_share),
                                    ))
                                    // Edit setup — installed plugins with setup values.
                                    .child(when(
                                        move || {
                                            store()
                                                .plugin_details
                                                .get()
                                                .get(&n_setup)
                                                .map(|d| d.installed && !d.fields.is_empty())
                                                .unwrap_or(false)
                                        },
                                        move || {
                                            let n = n_setup2.clone();
                                            pill_btn(None, || "Setup".to_owned(), move || store().open_setup(&n, "edit"))
                                        },
                                    ))
                                    .child(install_btn(name.clone())),
                            ),
                    )
                    // description — detail's, else the catalog row's.
                    .child(
                        text(move || {
                            let st = store();
                            let dd = st.plugin_details.get().get(&n_desc).cloned();
                            dd.map(|d| d.description)
                                .filter(|d| !d.is_empty())
                                .or_else(|| {
                                    st.plugins
                                        .get()
                                        .iter()
                                        .find(|p| p.name == n_desc)
                                        .map(|p| p.description.clone())
                                })
                                .unwrap_or_default()
                        })
                        .text_size(px(14.0))
                        .line_height(px(20.0))
                        .text_color(ink2())
                        .pt(px(12.0)),
                    )
                    .child(
                        view()
                            .pt(px(24.0))
                            .gap(px(16.0))
                            .child(accounts_section(n_acc))
                            .child(plugin_tools_section(n_tools))
                            .child(apps_section(n_apps))
                            .child(skills_section(n_skills))
                            .child(info_section(&name)),
                    ),
            ),
        )
}

/// Add / Uninstall — `installed` read live off the detail.
fn install_btn(name: String) -> impl IntoElement {
    let n = name.clone();
    let n_un = name.clone();
    let n_in = name;
    when(
        move || {
            store()
                .plugin_details
                .get()
                .get(&n)
                .map(|d| d.installed)
                .unwrap_or(false)
        },
        move || {
            let n_un = n_un.clone();
            let n_lab = n_un.clone();
            pill_btn(
                None,
                move || {
                    if store().plugin_busy.get() == n_lab {
                        "Removing…".to_owned()
                    } else {
                        "Uninstall".to_owned()
                    }
                },
                move || store().uninstall_plugin(&n_un),
            )
        },
    )
    .otherwise(move || {
        let n_in = n_in.clone();
        let n_lab = n_in.clone();
        let hov = signal(false);
        view()
            .h(px(36.0))
            .rounded(px(999.0))
            .px(px(16.0))
            .items_center()
            .justify_center()
            .bg(move || if hov.get() { pair("#363636", "#e0e0e0") } else { primary_bg() })
            .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
            .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
            .on_press(move || store().install_plugin(&n_in))
            .child(
                text(move || {
                    if store().plugin_busy.get() == n_lab {
                        "Adding…".to_owned()
                    } else {
                        "Add".to_owned()
                    }
                })
                .text_size(px(14.0))
                .line_height(px(20.0))
                .text_color(primary_ink()),
            )
    })
}

/// The plugin's remote (OAuth) connectors — the ones with accounts.
fn remote_connectors(plugin: &str) -> Vec<crate::store::PluginMcp> {
    store()
        .plugin_details
        .get()
        .get(plugin)
        .map(|d| d.mcp.iter().filter(|m| m.transport == "remote").cloned().collect())
        .unwrap_or_default()
}

/// Accounts — one card per remote connector of an added plugin: an account row each (a
/// not-yet-registered connector shows its "default" slot), then "Add
/// Another Account". Rebuilt on detail / edit-state changes only.
fn accounts_section(plugin: String) -> impl IntoElement {
    dynamic(move || {
        // Accounts belong to an added plugin; before that the Apps list
        // already names its connectors (Grok Bot does the same).
        let installed = store().plugin_details.get().get(&plugin).is_some_and(|d| d.installed);
        let conns = if installed { remote_connectors(&plugin) } else { Vec::new() };
        if conns.is_empty() {
            return view().into_element();
        }
        let many = conns.len() > 1;
        let mut col = view().gap(px(16.0));
        for m in conns {
            let label = if many {
                format!("Accounts · {}", m.name)
            } else {
                "Accounts".to_owned()
            };
            if !m.unavailable.is_empty() {
                col = col.child(
                    view().child(sec_label(label)).child(
                        card().py(px(8.0)).child(
                            view()
                                .px(px(14.0))
                                .py(px(10.0))
                                .flex_row()
                                .items_center()
                                .gap(px(10.0))
                                .child(
                                    text(m.unavailable.clone())
                                        .flex_1()
                                        .text_size(px(13.0))
                                        .line_height(px(18.0))
                                        .text_color(ink3()),
                                )
                                .child(status_text("Unavailable", ink3())),
                        ),
                    ),
                );
                continue;
            }
            let mut accounts = m.accounts.clone();
            if accounts.is_empty() {
                accounts.push(crate::store::PluginAccount {
                    label: "default".into(),
                    status: "not-added".into(),
                    note: if m.needs.is_empty() {
                        String::new()
                    } else {
                        format!("Add {} in Setup to connect", m.needs.join(", "))
                    },
                    ..Default::default()
                });
            }
            let mut c = card().py(px(8.0));
            for (i, a) in accounts.into_iter().enumerate() {
                c = c.child(account_row(&plugin, &m.name, a, i == 0));
            }
            c = c.child(add_account(&plugin, &m.name));
            col = col.child(view().child(sec_label(label)).child(c));
        }
        col.into_element()
    })
}

fn account_row(
    plugin: &str,
    server: &str,
    a: crate::store::PluginAccount,
    is_default: bool,
) -> AnyElement {
    let st = store();
    let editing = !a.id.is_empty() && st.acct_edit.get() == a.id;
    let confirm = st.acct_confirm.get();
    let busy = st.plugin_busy.get();
    let err = st.acct_err.get();
    let (plugin, server) = (plugin.to_owned(), server.to_owned());
    if editing {
        let (p1, p2, p3) = (plugin.clone(), plugin.clone(), plugin.clone());
        let (id1, id2, id3) = (a.id.clone(), a.id.clone(), a.id.clone());
        let confirming = confirm == a.id;
        let removable = !is_default;
        let signed_in = a.authed;
        let id4 = a.id.clone();
        let mut row = view()
            .flex_row()
            .items_center()
            .gap(px(7.0))
            .px(px(14.0))
            .py(px(6.0))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .h(px(26.0))
                    .rounded(px(8.0))
                    .bg(tc(0.04))
                    .px(px(8.0))
                    .justify_center()
                    .child(
                        input()
                            .value(move || store().acct_edit_val.get())
                            .placeholder("Account label")
                            .placeholder_color(|| color(tc(0.4)))
                            .auto_focus(true)
                            .text_size(px(13.0))
                            .text_color(theme::TEXT)
                            .h(px(26.0))
                            .on_change(|t: &str| store().acct_edit_val.set(t.to_owned()))
                            .on_submit(move |_: &str| {
                                let v = store().acct_edit_val.get_untracked();
                                store().rename_account(&id1, &v, &p1)
                            })
                            .on_key_press(|k: &str| {
                                if k == "Escape" {
                                    store().acct_edit.set(String::new());
                                }
                            }),
                    ),
            )
            .child(sm_btn(Btn::Primary, || "Save".into(), move || {
                let v = store().acct_edit_val.get_untracked();
                store().rename_account(&id2, &v, &p2)
            }));
        if removable || signed_in {
            let verb = if removable { "Remove" } else { "Disconnect" };
            row = row.child(sm_btn(
                if confirming { Btn::DangerSolid } else { Btn::Danger },
                move || {
                    if confirming {
                        format!("Confirm {verb}")
                    } else {
                        verb.to_owned()
                    }
                },
                move || {
                    if !confirming {
                        store().acct_confirm.set(id3.clone());
                    } else if removable {
                        store().remove_account(&id3, &p3)
                    } else {
                        store().disconnect_account(&id3, &p3)
                    }
                },
            ));
        }
        let _ = id4;
        return row
            .child(sm_btn(Btn::Ghost, || "Cancel".into(), || {
                store().acct_edit.set(String::new());
                store().acct_confirm.set(String::new());
            }))
            .into_element();
    }
    let needs = Store::needs_auth(&a.err);
    let id = a.id.clone();
    // Setup values missing (e.g. the user's own OAuth Client ID): the row
    // says what, and Authenticate waits for them.
    let blocked: Vec<String> = st
        .plugin_details
        .get_untracked()
        .get(&plugin)
        .and_then(|d| d.mcp.iter().find(|m| m.name == server).map(|m| m.needs.clone()))
        .unwrap_or_default();
    let blocked = !blocked.is_empty();
    let connected_note = if a.status == "connected" && a.note.starts_with("Connected") { a.note.clone() } else { String::new() };
    let (status, ink): (String, AdaptiveColor) = match a.status.as_str() {
        "connected" if !connected_note.is_empty() => (connected_note.clone(), ok_ink()),
        "connected" => ("Connected".into(), ok_ink()),
        "not-added" | "needs-auth" => ("Needs auth".into(), warn_ink()),
        "error" if needs => ("Needs auth".into(), warn_ink()),
        "error" => ("Error".into(), danger_ink()),
        "off" => ("Disconnected".into(), ink3()),
        _ => ("Starting".into(), ink3()),
    };
    // the action beside the status: Authenticate (OAuth), Reconnect
    // (a failed probe), Connect (a disabled row).
    let action: Option<(&'static str, Box<dyn Fn()>)> = match a.status.as_str() {
        _ if blocked && a.status != "connected" => {
            let p = plugin.clone();
            Some(("Setup", Box::new(move || store().open_setup(&p, "edit"))))
        }
        "not-added" => {
            let (p, s) = (plugin.clone(), server.clone());
            Some(("Authenticate", Box::new(move || store().connect_account(&p, &s, ""))))
        }
        "needs-auth" => {
            let (p, s) = (plugin.clone(), server.clone());
            if is_default {
                Some(("Authenticate", Box::new(move || store().connect_account(&p, &s, ""))))
            } else {
                let i = id.clone();
                Some(("Authenticate", Box::new(move || store().auth_account(&i, &p))))
            }
        }
        "error" if needs => {
            let (p, i) = (plugin.clone(), id.clone());
            Some(("Authenticate", Box::new(move || store().auth_account(&i, &p))))
        }
        "error" => {
            let (p, i) = (plugin.clone(), id.clone());
            Some(("Reconnect", Box::new(move || store().retry_server(&i, &p))))
        }
        "off" if is_default => {
            let (p, s) = (plugin.clone(), server.clone());
            Some(("Connect", Box::new(move || store().connect_account(&p, &s, ""))))
        }
        "off" => {
            let i = id.clone();
            Some(("Connect", Box::new(move || store().set_server_enabled(&i, true))))
        }
        _ => None,
    };
    let busy_here = busy == format!("auth:{}", a.id) || (a.id.is_empty() && busy == format!("connect:{server}"));
    let label = a.label.clone();
    let edit_id = a.id.clone();
    let edit_label = a.label.clone();
    let mut right = view()
        .flex_row()
        .items_center()
        .gap(px(7.0))
        .child(status_text(&status, ink));
    if let Some((verb, f)) = action {
        right = right.child(sm_btn(
            Btn::Secondary,
            move || if busy_here { "Opening…".to_owned() } else { verb.to_owned() },
            move || f(),
        ));
    }
    let show_err = a.status == "error" && !needs && !a.err.is_empty();
    view()
        .px(px(14.0))
        .py(px(10.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .min_h(px(18.0))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex_row()
                        .items_center()
                        .gap(px(5.0))
                        .child(
                            text(label)
                                .number_of_lines(1)
                                .text_size(px(13.0))
                                .line_height(px(18.0))
                                .text_color(theme::TEXT),
                        )
                        .when(!edit_id.is_empty(), move |v| {
                            v.child(
                                view()
                                    .size(px(14.0))
                                    .items_center()
                                    .justify_center()
                                    .label("Edit account")
                                    .on_press(move || {
                                        store().acct_edit_val.set(edit_label.clone());
                                        store().acct_confirm.set(String::new());
                                        store().acct_edit.set(edit_id.clone());
                                    })
                                    .child(icon_ink("gb-pencil", 10.0, ink3)),
                            )
                        }),
                )
                .child(right),
        )
        .when(!a.note.is_empty() && !a.note.starts_with("Connected"), |v| {
            v.child(
                text(a.note.clone())
                    .number_of_lines(2)
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(ink3())
                    .pt(px(4.0)),
            )
        })
        .when(show_err, |v| {
            v.child(
                text(format!("Failed to load connector. Details: {}", a.err))
                    .number_of_lines(3)
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(ink3())
                    .pt(px(4.0)),
            )
        })
        .when(is_default && a.id.is_empty() && !err.is_empty() && store().acct_add.get_untracked().is_empty(), |v| {
            v.child(
                text(err.clone())
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(danger_ink())
                    .pt(px(4.0)),
            )
        })
        .into_element()
}

/// "+ Add Another Account" → the inline label form (Grok's: a label
/// field, Authorize, Cancel). Authorize registers the account and opens
/// its OAuth tab (`/plugins/connect`).
fn add_account(plugin: &str, server: &str) -> AnyElement {
    let st = store();
    let open = st.acct_add.get() == server;
    let (plugin, server) = (plugin.to_owned(), server.to_owned());
    if !open {
        let hov = signal(false);
        return view()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(14.0))
            .py(px(10.0))
            .bg(move || if hov.get() { tc(0.03) } else { AdaptiveColor::TRANSPARENT })
            .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
            .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
            .on_press(move || {
                store().acct_add_val.set(String::new());
                store().acct_err.set(String::new());
                store().acct_edit.set(String::new());
                store().acct_add.set(server.clone());
            })
            .child(icon_ink("gb-plus", 10.0, ink2))
            .child(
                text("Add Another Account")
                    .text_size(px(13.0))
                    .line_height(px(18.0))
                    .text_color(ink2()),
            )
            .into_element();
    }
    let busy = st.plugin_busy.get() == format!("connect:{server}");
    let err = st.acct_err.get();
    let (p1, s1, p2, s2) = (plugin.clone(), server.clone(), plugin, server);
    view()
        .px(px(14.0))
        .py(px(6.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(7.0))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .h(px(26.0))
                        .rounded(px(8.0))
                        .bg(tc(0.04))
                        .px(px(8.0))
                        .justify_center()
                        .child(
                            input()
                                .value(move || store().acct_add_val.get())
                                .placeholder("Label this account, e.g. work or personal")
                                .placeholder_color(|| color(tc(0.4)))
                                .auto_focus(true)
                                .text_size(px(13.0))
                                .text_color(theme::TEXT)
                                .h(px(26.0))
                                .on_change(|t: &str| store().acct_add_val.set(t.to_owned()))
                                .on_submit(move |_: &str| {
                                    let v = store().acct_add_val.get_untracked();
                                    if !v.trim().is_empty() {
                                        store().connect_account(&p1, &s1, &v)
                                    }
                                })
                                .on_key_press(|k: &str| {
                                    if k == "Escape" {
                                        store().acct_add.set(String::new());
                                    }
                                }),
                        ),
                )
                .child(sm_btn(
                    Btn::Primary,
                    move || if busy { "Opening…".into() } else { "Authorize".into() },
                    move || {
                        let v = store().acct_add_val.get_untracked();
                        if !v.trim().is_empty() {
                            store().connect_account(&p2, &s2, &v)
                        }
                    },
                ))
                .child(sm_btn(Btn::Ghost, || "Cancel".into(), || {
                    store().acct_add.set(String::new());
                    store().acct_err.set(String::new());
                })),
        )
        .when(!err.is_empty(), |v| {
            v.child(
                text(err.clone())
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(danger_ink())
                    .pt(px(4.0)),
            )
        })
        .into_element()
}

/// Tools — per connected connector: "N of M enabled ⌄" opening the
/// per-tool switches (Grok shows it once the connector is connected).
fn plugin_tools_section(plugin: String) -> impl IntoElement {
    each(
        move || {
            store()
                .plugin_details
                .get()
                .get(&plugin)
                .map(|d| {
                    d.mcp
                        .iter()
                        .filter(|m| !m.id.is_empty() && m.status == "connected")
                        .map(|m| m.id.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        },
        |id| id.clone(),
        |id, _| view().child(tools_section(id.get())),
    )
}

/// `toolSwitch` — Grok Bot's 26×16 switch: blue track, 12pt knob.
pub(crate) fn tool_switch(on: impl Fn() -> bool + 'static, on_toggle: impl Fn() + 'static) -> impl IntoElement {
    let on2 = std::rc::Rc::new(on);
    let on3 = on2.clone();
    view()
        .w(px(26.0))
        .h(px(16.0))
        .rounded(px(8.0))
        .justify_center()
        .bg(move || if (on2)() { pair("#1680f3", "#1e86fc") } else { tc(0.15) })
        .on_press(move || on_toggle())
        .child(
            view()
                .size(px(12.0))
                .rounded(px(6.0))
                .bg(white())
                .ml(move || if (on3)() { px(12.0) } else { px(2.0) }),
        )
}

/// "Tools" + the collapsible "N of M enabled" card (hidden until the
/// server's tools are known).
fn tools_section(server_id: String) -> impl IntoElement {
    let id = server_id.clone();
    let id_has = server_id.clone();
    let id2 = server_id.clone();
    let id3 = server_id.clone();
    let id4 = server_id.clone();
    let id5 = server_id;
    when(
        move || store().mcp_tools.get().get(&id_has).map(|t| !t.is_empty()).unwrap_or(false),
        move || {
            let (id, id2, id3, id4, id5) = (id.clone(), id2.clone(), id3.clone(), id4.clone(), id5.clone());
            let id_chev = id2.clone();
            view().child(sec_label("Tools")).child(
                card()
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .px(px(14.0))
                            .py(px(10.0))
                            .on_press(move || {
                                let open = store().tools_open.get_untracked().get(&id).copied().unwrap_or(false);
                                let k = id.clone();
                                store().tools_open.update(|m| {
                                    m.insert(k, !open);
                                });
                            })
                            .child(
                                text(move || {
                                    let tools = store().mcp_tools.get().get(&id2).cloned().unwrap_or_default();
                                    let on = tools.iter().filter(|t| t.enabled).count();
                                    format!("{} of {} enabled", on, tools.len())
                                })
                                .flex_1()
                                .text_size(px(13.0))
                                .line_height(px(18.0))
                                .text_color(theme::TEXT),
                            )
                            .child(dynamic(move || {
                                let open = store().tools_open.get().get(&id_chev).copied().unwrap_or(false);
                                icon_ink(if open { "gb-chevron-up" } else { "gb-chevron-down" }, 10.0, ink3)
                            })),
                    )
                    .child(when(
                        move || store().tools_open.get().get(&id3).copied().unwrap_or(false),
                        move || {
                            let (id4, id5) = (id4.clone(), id5.clone());
                            view().child(each(
                                move || store().mcp_tools.get().get(&id4).cloned().unwrap_or_default(),
                                |t| t.name.clone(),
                                move |t, _| {
                                    let sid = id5.clone();
                                    let name = t.get().name.clone();
                                    view()
                                        .flex_row()
                                        .items_center()
                                        .px(px(14.0))
                                        .py(px(10.0))
                                        .child(
                                            text(move || t.get().name)
                                                .number_of_lines(1)
                                                .flex_1()
                                                .text_size(px(13.0))
                                                .line_height(px(18.0))
                                                .text_color(theme::TEXT),
                                        )
                                        .child(tool_switch(
                                            move || t.get().enabled,
                                            move || {
                                                let on = t.get().enabled;
                                                store().toggle_mcp_tool(&sid, &name, !on)
                                            },
                                        ))
                                },
                            ))
                        },
                    )),
            )
        },
    )
}

fn apps_section(plugin: String) -> impl IntoElement {
    let n = plugin.clone();
    let n2 = plugin.clone();
    when(
        move || store().plugin_details.get().get(&n).map(|d| !d.mcp.is_empty()).unwrap_or(false),
        move || {
            let n2 = n2.clone();
            let n3 = n2.clone();
            view()
                .child(count_label("Apps", move || {
                    store().plugin_details.get().get(&n2).map(|d| d.mcp.len()).unwrap_or(0)
                }))
                .child(card().py(px(8.0)).child(each(
                    move || store().plugin_details.get().get(&n3).map(|d| d.mcp.clone()).unwrap_or_default(),
                    |m| m.name.clone(),
                    |m, _| glyph_row("gb-plug", m.get().name, "Connector".into()),
                )))
        },
    )
}

/// Skills — read-only rows (only private skills open a page); five,
/// then "N more".
fn skills_section(plugin: String) -> impl IntoElement {
    let n = plugin.clone();
    let n2 = plugin;
    when(
        move || store().plugin_details.get().get(&n).map(|d| !d.skills.is_empty()).unwrap_or(false),
        move || {
            let (a, b, c) = (n2.clone(), n2.clone(), n2.clone());
            let d2 = n2.clone();
            view()
                .child(count_label("Skills", move || {
                    store().plugin_details.get().get(&a).map(|d| d.skills.len()).unwrap_or(0)
                }))
                .child(
                    card()
                        .pt(px(8.0))
                        .pb(move || {
                            let many = store().plugin_details.get().get(&d2).map(|d| d.skills.len() > 5).unwrap_or(false);
                            if many { px(0.0) } else { px(8.0) }
                        })
                        .child(each(
                            move || {
                                let list = store().plugin_details.get().get(&b).map(|d| d.skills.clone()).unwrap_or_default();
                                if store().skills_expanded.get() {
                                    list
                                } else {
                                    list.into_iter().take(5).collect()
                                }
                            },
                            |s| s.name.clone(),
                            |s, _| {
                                let sk = s.get();
                                glyph_row("gb-cube", sk.name, sk.description)
                            },
                        ))
                        .child(when(
                            {
                                let c = c.clone();
                                move || store().plugin_details.get().get(&c).map(|d| d.skills.len() > 5).unwrap_or(false)
                            },
                            move || {
                                let c = c.clone();
                                let hov = signal(false);
                                view()
                                    .flex_row()
                                    .justify_end()
                                    .px(px(14.0))
                                    .py(px(10.0))
                                    .bg(move || if hov.get() { tc(0.03) } else { AdaptiveColor::TRANSPARENT })
                                    .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
                                    .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
                                    .on_press(|| store().skills_expanded.update(|v| *v = !*v))
                                    .child(
                                        text(move || {
                                            let n = store().plugin_details.get().get(&c).map(|d| d.skills.len()).unwrap_or(0);
                                            if store().skills_expanded.get() {
                                                "Show less".to_owned()
                                            } else {
                                                format!("{} more", n.saturating_sub(5))
                                            }
                                        })
                                        .text_size(px(13.0))
                                        .line_height(px(18.0))
                                        .text_color(ink3()),
                                    )
                            },
                        )),
                )
        },
    )
}

fn info_section(name: &str) -> impl IntoElement {
    let n_cap = name.to_owned();
    let n_dev = name.to_owned();
    let n_cat = name.to_owned();
    let n_web = name.to_owned();
    let n_av = name.to_owned();
    let detail = |n: &str| store().plugin_details.get().get(n).cloned().unwrap_or_default();
    view().child(sec_label("Information")).child(
        card()
            .py(px(8.0))
            .child(info_row("Capabilities", move || {
                let d = detail(&n_cap);
                format!(
                    "{} app{}, {} skill{}",
                    d.mcp.len(),
                    if d.mcp.len() == 1 { "" } else { "s" },
                    d.skills.len(),
                    if d.skills.len() == 1 { "" } else { "s" },
                )
            }))
            .child(info_row("Developer", move || {
                let d = detail(&n_dev);
                if !d.author.is_empty() {
                    d.author
                } else {
                    developer_of(&d.homepage)
                }
            }))
            .child(info_row("Category", move || Store::plugin_title(&detail(&n_cat).category)))
            .child(info_row("Website", move || host(&detail(&n_web).homepage)))
            .child(info_row("Availability", move || {
                let a = detail(&n_av).availability;
                if a.is_empty() { "Public".into() } else { a }
            })),
    )
}

/// `new URL(homepage).host` — else "—".
fn host(url: &str) -> String {
    url.strip_prefix("http")
        .and_then(|r| r.split("://").nth(1))
        .and_then(|r| r.split('/').next())
        .map(|s| s.to_owned())
        .unwrap_or_else(|| "—".to_owned())
}

/// The Developer fallback: a GitHub homepage's owner ("vercel" →
/// "Vercel"), else the host's first label.
fn developer_of(url: &str) -> String {
    let h = host(url);
    if h == "github.com" {
        if let Some(owner) = url.split("github.com/").nth(1).and_then(|r| r.split('/').next()) {
            if !owner.is_empty() {
                return Store::plugin_title(owner);
            }
        }
    }
    h.split('.').next().map(|s| s.to_owned()).unwrap_or_else(|| "—".to_owned())
}

// ═══ SETUP page — a plugin's setup values ═══
//
// Grok Bot's plugin setup form (measured): the detail bar titled with the
// plugin, a 14/20 secondary intro, fields 14pt apart — 13/18 medium label
// (+ " (optional)" tertiary), a 32-tall 8-radius 4% input (10pt inset,
// 13pt), a 12/16 tertiary hint — then the 26-tall primary button at the
// right ("Add <Plugin>" / "Save Values"). Secrets never come back: a
// stored one shows dots as its placeholder.

fn setup_page() -> impl IntoElement {
    let top = store().plugin_nav.get_untracked().last().cloned();
    let (name, mode) = top.map(|n| (n.name, n.id)).unwrap_or_default();
    let install = mode == "install";
    let title = store().plugin_display(&name);
    let n_body = name.clone();
    let n_btn = name.clone();
    let title2 = title.clone();
    view()
        .flex_1()
        .child(back_bar(None, &title))
        .child(
            scroll().flex_1().child(
                pane()
                    .gap(px(16.0))
                    .child(
                        text(if install {
                            format!("{title2} needs a few values before it can be added")
                        } else {
                            format!(
                                "Saving replaces {title2}'s setup values and applies them to its connectors. Stored secrets show as dots; leave one blank to keep it."
                            )
                        })
                        .text_size(px(14.0))
                        .line_height(px(20.0))
                        .text_color(ink2()),
                    )
                    .child(dynamic(move || {
                        let d = store().plugin_details.get().get(&n_body).cloned().unwrap_or_default();
                        let mut col = view().gap(px(14.0));
                        if d.fields.iter().any(|f| f.shared == "google") {
                            col = col.child(
                                text("The Google Client ID and Secret are shared by every Google plugin — Gmail, Google Calendar, Drive, Docs, Sheets and Slides.")
                                    .text_size(px(12.0))
                                    .line_height(px(16.0))
                                    .text_color(ink3()),
                            );
                        }
                        for f in d.fields.clone() {
                            col = col.child(setup_field(f));
                        }
                        for m in d.mcp.iter().filter(|m| !m.callback_url.is_empty()) {
                            col = col.child(callback_row(m.callback_url.clone()));
                        }
                        col.into_element()
                    }))
                    .child(when(
                        || !store().setup_err.get().is_empty(),
                        || {
                            text(|| store().setup_err.get())
                                .text_size(px(12.0))
                                .line_height(px(16.0))
                                .text_color(danger_ink())
                        },
                    ))
                    .child(
                        view().flex_row().justify_end().child(sm_btn(
                            Btn::Primary,
                            move || {
                                if !store().plugin_busy.get().is_empty() {
                                    if install { "Adding…".into() } else { "Saving…".into() }
                                } else if install {
                                    format!("Add {title}")
                                } else {
                                    "Save Values".into()
                                }
                            },
                            move || store().submit_setup(&n_btn, install),
                        )),
                    ),
            ),
        )
}

fn setup_field(f: crate::store::PluginField) -> impl IntoElement {
    let key = f.key.clone();
    let mut label = view().flex_row().child(
        text(f.label.clone())
            .text_size(px(13.0))
            .line_height(px(18.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme::TEXT),
    );
    if !f.required {
        label = label.child(
            text(" (optional)")
                .text_size(px(13.0))
                .line_height(px(18.0))
                .text_color(ink3()),
        );
    }
    let control = if !f.options.is_empty() {
        select_box(f.clone()).into_element()
    } else {
        let (k1, k2, k3) = (key.clone(), key.clone(), key.clone());
        let stored = f.value.clone();
        let secret = f.secret;
        let placeholder = if secret && f.set { "••••••••".to_owned() } else { f.placeholder.clone() };
        view()
            .h(px(32.0))
            .rounded(px(8.0))
            .bg(tc(0.04))
            .px(px(10.0))
            .justify_center()
            .child(
                input()
                    .value(move || store().setup_vals.get().get(&k1).cloned().unwrap_or_else(|| stored.clone()))
                    .placeholder(placeholder)
                    .placeholder_color(|| color(tc(0.4)))
                    .text_size(px(13.0))
                    // secrets: the typed text hides under dots.
                    .text_color(move || if secret { AdaptiveColor::TRANSPARENT } else { theme::TEXT })
                    .h(px(32.0))
                    .on_change(move |t: &str| {
                        let (k, t) = (k2.clone(), t.to_owned());
                        store().setup_vals.update(|m| {
                            m.insert(k, t);
                        });
                    }),
            )
            .when(secret, move |v| {
                v.child(
                    text(move || {
                        let n = store().setup_vals.get().get(&k3).map(|s| s.chars().count()).unwrap_or(0);
                        "•".repeat(n)
                    })
                    .absolute()
                    .left(px(10.0))
                    .text_size(px(13.0))
                    .line_height(px(18.0))
                    .text_color(theme::TEXT)
                    .pointer_events(PointerEvents::None),
                )
            })
            .into_element()
    };
    let mut col = view().gap(px(4.0)).child(label).child(control);
    if !f.hint.is_empty() {
        col = col.child(
            text(f.hint.clone())
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_color(ink3()),
        );
    }
    col
}

/// A select (catalog `options`): the chosen label or "Select <label>",
/// opening a menu of the options.
fn select_box(f: crate::store::PluginField) -> impl IntoElement {
    let node = element_ref();
    let (k1, k2) = (f.key.clone(), f.key.clone());
    let opts = f.options.clone();
    let opts2 = f.options.clone();
    let label = f.label.clone();
    let stored = f.value.clone();
    view()
        .element_ref(&node)
        .h(px(32.0))
        .rounded(px(8.0))
        .bg(tc(0.04))
        .px(px(10.0))
        .flex_row()
        .items_center()
        .on_press(move || {
            let (k, opts) = (k2.clone(), opts2.clone());
            spawn(async move {
                let m = node.measure().await;
                let (x, y) = m.map(|m| (m.page.x.0, m.page.y.0 + m.frame.size.height.0 + 4.0)).unwrap_or((0.0, 0.0));
                let cur = store().setup_vals.get_untracked().get(&k).cloned().unwrap_or_default();
                let items: Vec<crate::store::CtxItem> = opts
                    .iter()
                    .map(|(l, v)| {
                        let mut it = crate::store::CtxItem::row(l, "", "setup-opt", format!("{k}\u{1f}{v}"));
                        it.checked = &cur == v;
                        it
                    })
                    .collect();
                crate::ctx::open_at(x, y, items);
            });
        })
        .child(
            text(move || {
                let cur = store().setup_vals.get().get(&k1).cloned().unwrap_or_else(|| stored.clone());
                opts.iter()
                    .find(|(_, v)| *v == cur)
                    .map(|(l, _)| l.clone())
                    .unwrap_or_else(|| format!("Select {label}"))
            })
            .flex_1()
            .text_size(px(13.0))
            .line_height(px(18.0))
            .text_color(theme::TEXT),
        )
        .child(icon_ink("gb-chevron-down", 10.0, ink3))
}

/// The fixed OAuth callback to register in the provider's developer
/// portal — read-only, with Copy.
fn callback_row(url: String) -> impl IntoElement {
    let u2 = url.clone();
    let copied = signal(false);
    view()
        .gap(px(4.0))
        .child(
            text("Callback URL")
                .text_size(px(13.0))
                .line_height(px(18.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::TEXT),
        )
        .child(
            view()
                .h(px(32.0))
                .rounded(px(8.0))
                .bg(tc(0.04))
                .pl(px(10.0))
                .pr(px(4.0))
                .flex_row()
                .items_center()
                .child(
                    text(url)
                        .flex_1()
                        .number_of_lines(1)
                        .text_size(px(13.0))
                        .line_height(px(18.0))
                        .text_color(theme::TEXT)
                        .selectable(true),
                )
                .child(sm_btn(
                    Btn::Ghost,
                    move || if copied.get() { "Copied".into() } else { "Copy".into() },
                    move || {
                        store().copy_text(&u2);
                        copied.set(true);
                    },
                )),
        )
        .child(
            text("Register this exact URL as the redirect / callback URL of your OAuth app.")
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_color(ink3()),
        )
}

// ═══ MCP server detail page ═══

fn mcp_page() -> impl IntoElement {
    let id = store()
        .plugin_nav
        .get_untracked()
        .last()
        .map(|n| n.id.clone())
        .unwrap_or_default();
    let s = store()
        .mcp_servers
        .get_untracked()
        .iter()
        .find(|x| x.id == id)
        .cloned();
    let Some(s) = s else {
        return view().flex_1().child(back_bar(None, "")).into_element();
    };
    let title = if s.name.is_empty() {
        s.id.clone()
    } else {
        s.name.clone()
    };
    let initial = title
        .chars()
        .next()
        .map(|c| c.to_ascii_uppercase().to_string())
        .unwrap_or_default();
    let remote = !s.url.is_empty();
    let url_or_cmd = if remote {
        s.url.clone()
    } else {
        format!("{} {}", s.command, s.args.join(" ")).trim().to_owned()
    };
    let url_label = if remote { "URL" } else { "Command" };
    let sid = s.id.clone();
    let sid_acc = s.id.clone();
    let sid_tools = s.id.clone();
    let sid_count = s.id.clone();
    view()
        .flex_1()
        .child(back_bar(None, &title))
        .child(
            scroll().flex_1().child(
                pane()
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .h(px(56.0))
                            // `sand-tool-icon--mono`: the letter on a 10% wash.
                            .child(
                                view()
                                    .size(px(56.0))
                                    .rounded(px(16.0))
                                    .bg(tc(0.10))
                                    .border(px(0.5))
                                    .border_color(tc(0.08))
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        text(initial)
                                            .text_size(px(14.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(ink2()),
                                    ),
                            )
                            .child(
                                text(title.clone())
                                    .number_of_lines(1)
                                    .flex_1()
                                    .ml(px(12.0))
                                    .text_size(px(17.0))
                                    .line_height(px(24.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme::TEXT),
                            )
                            .child(pill_btn(None, || "Uninstall".into(), move || {
                                let id = sid.clone();
                                spawn(async move {
                                    let _ = crate::client::post(
                                        "/mcp/server/delete",
                                        serde_json::json!({"id": id}),
                                    )
                                    .await;
                                    store().fetch_mcp();
                                    store().pop_nav();
                                });
                            })),
                    )
                    .child(
                        view()
                            .pt(px(24.0))
                            .gap(px(16.0))
                            .child(view().child(sec_label("Accounts")).child(dynamic(move || {
                                let row = store()
                                    .mcp_servers
                                    .get()
                                    .iter()
                                    .find(|x| x.id == sid_acc)
                                    .cloned()
                                    .unwrap_or_default();
                                server_account(row).into_element()
                            })))
                            .child(when(
                                {
                                    let id = sid_tools.clone();
                                    move || {
                                        store().mcp_servers.get().iter().any(|x| x.id == id && x.status == "connected")
                                    }
                                },
                                move || view().child(tools_section(sid_tools.clone())),
                            ))
                            .child(
                                view().child(sec_label("Details")).child(
                                    card()
                                        .py(px(8.0))
                                        .child(info_row("Source", || "Added manually".into()))
                                        .child(info_row("Transport", move || {
                                            if remote { "Remote" } else { "Command" }.to_owned()
                                        }))
                                        .child(info_row(url_label, move || url_or_cmd.clone()))
                                        .child(info_row("Tools", move || {
                                            // the listed tools when known, else
                                            // the server's last discovered count.
                                            let st = store();
                                            let n = st.mcp_tools.get().get(&sid_count).map(|t| t.len());
                                            let n = n.filter(|n| *n > 0).unwrap_or_else(|| {
                                                st.mcp_servers
                                                    .get()
                                                    .iter()
                                                    .find(|x| x.id == sid_count)
                                                    .map(|x| x.tools.max(0) as usize)
                                                    .unwrap_or(0)
                                            });
                                            // nothing discovered yet (never probed / off).
                                            if n == 0 { "—".to_owned() } else { n.to_string() }
                                        })),
                                ),
                            ),
                    ),
            ),
        )
        .into_element()
}

/// A standalone server's one "default" account row: its live status and
/// the action that fixes it (Authenticate / Reconnect / Connect).
fn server_account(s: McpServer) -> impl IntoElement {
    let needs = Store::needs_auth(&s.err);
    let remote = !s.url.is_empty();
    let (status, ink): (&str, AdaptiveColor) = match s.status.as_str() {
        "connected" => ("Connected", ok_ink()),
        "error" if needs && remote => ("Needs auth", warn_ink()),
        "error" => ("Error", danger_ink()),
        "off" => ("Disconnected", ink3()),
        _ => ("Starting", ink3()),
    };
    let id = s.id.clone();
    let transport = if remote { "remote" } else { "command" }.to_owned();
    let err = s.err.clone();
    let action: Option<(&'static str, Box<dyn Fn()>)> = match s.status.as_str() {
        "error" => {
            let verb = if needs && remote { "Authenticate" } else { "Reconnect" };
            Some((verb, Box::new(move || store().auth_or_retry(&id, &transport, &err, ""))))
        }
        "off" => Some(("Connect", Box::new(move || store().set_server_enabled(&id, true)))),
        _ => None,
    };
    let mut right = view().flex_row().items_center().gap(px(7.0)).child(status_text(status, ink));
    if let Some((verb, f)) = action {
        right = right.child(sm_btn(Btn::Secondary, move || verb.to_owned(), move || f()));
    }
    let show_err = s.status == "error" && !(needs && remote) && !s.err.is_empty();
    card().py(px(8.0)).child(
        view()
            .px(px(14.0))
            .py(px(10.0))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .min_h(px(18.0))
                    .child(
                        text("default")
                            .flex_1()
                            .text_size(px(13.0))
                            .line_height(px(18.0))
                            .text_color(theme::TEXT),
                    )
                    .child(right),
            )
            .when(show_err, |v| {
                v.child(
                    text(format!("Failed to load connector. Details: {}", s.err))
                        .number_of_lines(3)
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .text_color(ink3())
                        .pt(px(4.0)),
                )
            }),
    )
}

// ═══ SKILL page — a private skill (plugin skills don't open a page) ═══

fn skill_page() -> impl IntoElement {
    let nav = store()
        .plugin_nav
        .get_untracked()
        .last()
        .cloned()
        .unwrap_or_else(PluginNav::market);
    let title = store()
        .private_skills
        .get_untracked()
        .iter()
        .find(|s| s.id == nav.id)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "New Skill".to_owned());
    view()
        .flex_1()
        .child(back_bar(None, &title))
        .child(private_skill_editor())
}

/// The saved copy of the skill being edited (None = a new skill).
fn saved_skill() -> Option<crate::store::PrivateSkill> {
    let id = store().ps_id.get();
    if id.is_empty() {
        return None;
    }
    store().private_skills.get().iter().find(|s| s.id == id).cloned()
}

/// Save is live once the draft differs from what's saved (a new skill:
/// once it has a name and instructions).
fn skill_dirty() -> bool {
    let st = store();
    let (n, d, b) = (st.ps_name.get(), st.ps_desc.get(), st.ps_body.get());
    if n.trim().is_empty() || b.trim().is_empty() {
        return false;
    }
    match saved_skill() {
        Some(s) => s.name != n || s.description != d || s.body != b,
        None => true,
    }
}

/// A labeled filled field (`Name` / `Description` / `Instructions`):
/// 12pt label with 8/8/6 padding over a 4% wash, radius 14, 12/16 inset.
fn skill_field(label: &'static str, field: impl IntoElement, h: f32) -> impl IntoElement {
    view()
        .child(
            text(label)
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_color(ink3())
                .pt(px(8.0))
                .pb(px(6.0))
                .px(px(8.0)),
        )
        .child(
            view()
                .h(px(h))
                .rounded(px(14.0))
                .bg(tc(0.04))
                .px(px(16.0))
                .child(field),
        )
}

/// The private skill editor (Grok Bot's `sand-plugins-skill-detail`):
/// header tile + name + "Private skill" + Delete Skill, the description,
/// the three fields, and a footer bar with Save.
fn private_skill_editor() -> impl IntoElement {
    view()
        .flex_1()
        .child(
            scroll().flex_1().child(
                pane()
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .h(px(56.0))
                            .child(
                                view()
                                    .size(px(56.0))
                                    .rounded(px(6.0))
                                    .bg(tc(0.03))
                                    .border(px(0.5))
                                    .border_color(tc(0.08))
                                    .items_center()
                                    .justify_center()
                                    .child(icon_ink("gb-file-list", 18.0, || tc(0.92))),
                            )
                            .child(
                                view()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .ml(px(12.0))
                                    .gap(px(2.0))
                                    .child(
                                        text(move || {
                                            saved_skill()
                                                .map(|s| s.name)
                                                .unwrap_or_else(|| "New Skill".to_owned())
                                        })
                                        .number_of_lines(1)
                                        .text_size(px(17.0))
                                        .line_height(px(24.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme::TEXT),
                                    )
                                    .child(
                                        text("Private skill")
                                            .text_size(px(13.0))
                                            .line_height(px(16.0))
                                            .text_color(ink2()),
                                    ),
                            )
                            .child(when(
                                || !store().ps_id.get().is_empty(),
                                || {
                                    view().ml(px(12.0)).child(sm_btn(
                                        Btn::Danger,
                                        || {
                                            if store().acct_confirm.get() == format!("skill:{}", store().ps_id.get()) {
                                                "Confirm Delete".to_owned()
                                            } else {
                                                "Delete Skill".to_owned()
                                            }
                                        },
                                        || {
                                            let key = format!("skill:{}", store().ps_id.get_untracked());
                                            if store().acct_confirm.get_untracked() == key {
                                                store().acct_confirm.set(String::new());
                                                store().delete_private_skill()
                                            } else {
                                                store().acct_confirm.set(key)
                                            }
                                        },
                                    ))
                                },
                            )),
                    )
                    // the saved description under the header.
                    .child(when(
                        || saved_skill().map(|s| !s.description.is_empty()).unwrap_or(false),
                        || {
                            text(|| saved_skill().map(|s| s.description).unwrap_or_default())
                                .text_size(px(14.0))
                                .line_height(px(20.0))
                                .text_color(ink2())
                                .pt(px(12.0))
                        },
                    ))
                    .child(
                        view()
                            .pt(px(24.0))
                            .gap(px(16.0))
                            .child(skill_field(
                                "Name",
                                input()
                                    .value(move || store().ps_name.get())
                                    .placeholder("skill-name")
                                    .placeholder_color(|| color(tc(0.3)))
                                    .text_size(px(14.0))
                                    .text_color(theme::TEXT)
                                    .h(px(46.0))
                                    .on_change(|t: &str| store().ps_name.set(t.to_owned())),
                                46.0,
                            ))
                            .child(skill_field(
                                "Description",
                                input()
                                    .value(move || store().ps_desc.get())
                                    .placeholder("When to use this skill")
                                    .placeholder_color(|| color(tc(0.3)))
                                    .text_size(px(14.0))
                                    .text_color(theme::TEXT)
                                    .h(px(46.0))
                                    .on_change(|t: &str| store().ps_desc.set(t.to_owned())),
                                46.0,
                            ))
                            .child(skill_field(
                                "Instructions",
                                input()
                                    .value(move || store().ps_body.get())
                                    .placeholder("What the agent should do, in markdown")
                                    .placeholder_color(|| color(tc(0.3)))
                                    .multiline(true)
                                    .text_size(px(14.0))
                                    .line_height(px(20.0))
                                    .text_color(theme::TEXT)
                                    .h(px(296.0))
                                    .mt(px(12.0))
                                    .on_change(|t: &str| store().ps_body.set(t.to_owned())),
                                320.0,
                            )),
                    ),
            ),
        )
        // footer: a hairline bar with Save right-aligned.
        .child(
            view()
                .flex_row()
                .justify_end()
                .border_t(px(0.5))
                .border_t_color(tc(0.12))
                .pt(px(12.0))
                .pb(px(12.0))
                .pl(px(16.0))
                .pr(px(12.0))
                .child(
                    view()
                        .h(px(32.0))
                        .rounded(px(999.0))
                        .px(px(12.0))
                        .items_center()
                        .justify_center()
                        .bg(move || if skill_dirty() { primary_bg() } else { tc(0.15) })
                        .on_press(|| {
                            if skill_dirty() {
                                store().save_private_skill()
                            }
                        })
                        .child(
                            text("Save")
                                .text_size(px(14.0))
                                .line_height(px(20.0))
                                .text_color(move || if skill_dirty() { primary_ink() } else { tc(0.3) }),
                        ),
                ),
        )
}


// ——————————————— v1 mini app plugins (spec 13) ———————————————

/// Connect Apps' v1 plugins: each with its state, version and publisher;
/// Add… inspects a plugin's command and shows its review.
fn v1_section() -> impl IntoElement {
    store().v1_refresh(false);
    view()
        .pb(px(14.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .pb_2()
                .child(text("Mini app plugins").text_size(px(12.0)).text_color(theme::DIM).pl(px(2.0)))
                .child(sm_btn(Btn::Secondary, || "Add…".to_owned(), || {
                    store().v1.review.set(None);
                    store().push_nav("v1install", "", "");
                })),
        )
        .child(each(
            || store().v1.listed.get(),
            |p| format!("{}|{}|{}", p.id, p.state, p.version),
            |p, _| v1_row(p.get_untracked()),
        ))
}

fn v1_row(p: crate::mini_v1::connect::Listed) -> impl IntoElement {
    let id = p.id.clone();
    let status = if p.failed() { danger_ink() } else { ink3() };
    card()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(14.0))
        .py(px(10.0))
        .mb(px(8.0))
        .label(p.title.clone())
        .child(plug_tile(&p.title, 32.0))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(text(p.title.clone()).number_of_lines(1).text_size(px(13.0)).line_height(px(18.0)).text_color(theme::TEXT))
                .child(
                    text(format!("{} · {}", p.version, if p.publisher.is_empty() { p.id.clone() } else { p.publisher.clone() }))
                        .number_of_lines(1)
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .text_color(ink3()),
                ),
        )
        .child(text(p.state_label()).text_size(px(12.0)).text_color(status))
        .on_press(move || store().push_nav("v1plugin", &id, ""))
}

/// The plugin on the nav stack's top, as listed now.
fn v1_current() -> Option<crate::mini_v1::connect::Listed> {
    let id = store().plugin_nav.get().last().map(|n| n.name.clone()).unwrap_or_default();
    store().v1.listed.get().into_iter().find(|p| p.id == id)
}

/// A v1 plugin's page: state (a failed plugin's Retry and Open logs),
/// Disable/Enable, Uninstall, its apps, the per-tool switches and its
/// routines (approve once, then enable or disable).
fn v1_plugin_page() -> impl IntoElement {
    let confirm = signal(false);
    view()
        .flex_1()
        .child(back_bar(None, &v1_current().map(|p| p.title).unwrap_or_default()))
        .child(scroll().flex_1().child(dynamic(move || {
            let Some(p) = v1_current() else {
                return pane().child(text("This plugin is not installed.").text_size(px(13.0)).text_color(ink3())).into_element();
            };
            let (id1, id2, id3, id4, id5) = (p.id.clone(), p.id.clone(), p.id.clone(), p.id.clone(), p.id.clone());
            let enabled = p.enabled;
            let mut head = view()
                .gap(px(2.0))
                .child(text(p.title.clone()).text_size(px(17.0)).font_weight(FontWeight::MEDIUM).text_color(theme::TEXT))
                .child(
                    text(format!("{} · {} · {}", p.version, p.publisher, p.id))
                        .text_size(px(12.0))
                        .text_color(ink3()),
                );
            // for a day after its version changed (inline cards just reload)
            let now = crate::mini_v1::session::now_ms();
            if let Some(updated) = store().v1.records.with(|r| r.updated(&p.id, now)) {
                head = head.child(text(updated).text_size(px(12.0)).text_color(ink3()));
            }
            let mut page = pane().gap(px(14.0)).child(
                head.child(
                    text(p.state_label())
                        .text_size(px(13.0))
                        .text_color(if p.failed() { danger_ink() } else { theme::TEXT }),
                ),
            );
            if p.failed() {
                page = page.child(
                    card()
                        .gap(px(8.0))
                        .p(px(14.0))
                        .child(text(if p.message.is_empty() { p.code.clone() } else { p.message.clone() }).text_size(px(13.0)).text_color(theme::TEXT))
                        .child(
                            view()
                                .flex_row()
                                .gap(px(8.0))
                                .child(sm_btn(Btn::Primary, || "Retry".to_owned(), move || store().v1_set_enabled(&id1, true)))
                                .child(sm_btn(Btn::Secondary, || "Open logs".to_owned(), move || store().v1_open_logs(&id2))),
                        ),
                );
            }
            if p.state == "update_available" {
                page = page.child(
                    text("A new version is ready. Add it again from its command to review what changed before it updates.")
                        .text_size(px(13.0))
                        .text_color(ink3()),
                );
            }
            page = page.child(
                view()
                    .flex_row()
                    .gap(px(8.0))
                    .child(sm_btn(
                        Btn::Secondary,
                        move || if enabled { "Disable".to_owned() } else { "Enable".to_owned() },
                        move || store().v1_set_enabled(&id3, !enabled),
                    ))
                    .child(when(
                        move || !confirm.get(),
                        move || sm_btn(Btn::Danger, || "Uninstall…".to_owned(), move || confirm.set(true)).into_element(),
                    ))
                    .child(when(
                        move || confirm.get(),
                        move || {
                            let id = id4.clone();
                            view()
                                .flex_row()
                                .gap(px(8.0))
                                .child(sm_btn(Btn::DangerSolid, || "Uninstall: remove its pins, attachments and data".to_owned(), move || {
                                    confirm.set(false);
                                    store().v1_uninstall(&id);
                                }))
                                .child(sm_btn(Btn::Ghost, || "Cancel".to_owned(), move || confirm.set(false)))
                                .into_element()
                        },
                    ))
                    .child(text(move || store().v1.busy.get()).text_size(px(12.0)).text_color(ink3())),
            );
            if !p.apps.is_empty() {
                let mut apps = card().child(sec_label("Apps"));
                for a in &p.apps {
                    apps = apps.child(text(a.clone()).px(px(14.0)).py(px(6.0)).text_size(px(13.0)).text_color(theme::TEXT));
                }
                page = page.child(apps);
            }
            if !p.tools.is_empty() {
                let mut tools = card().child(sec_label(format!("Tools {} of {} on", p.tools.iter().filter(|t| !t.disabled).count(), p.tools.len())));
                for t in p.tools.clone() {
                    let (plugin, name, on) = (id5.clone(), t.name.clone(), !t.disabled);
                    tools = tools.child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(10.0))
                            .px(px(14.0))
                            .py(px(8.0))
                            .child(
                                view()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .child(text(t.name.clone()).text_size(px(13.0)).text_color(theme::TEXT))
                                    .child(text(t.description.clone()).number_of_lines(2).text_size(px(12.0)).text_color(ink3())),
                            )
                            .child(tool_switch(move || on, move || store().v1_set_tool(&plugin, &name, on))),
                    );
                }
                page = page.child(tools);
            }
            if !p.routines.is_empty() {
                let mut routines = card().child(sec_label("Routines"));
                for r in p.routines.clone() {
                    let plugin = p.id.clone();
                    let (rid1, rid2, rid3) = (r.id.clone(), r.id.clone(), r.id.clone());
                    let (p1, p2, p3) = (plugin.clone(), plugin.clone(), plugin);
                    let mut row = view().gap(px(4.0)).px(px(14.0)).py(px(10.0)).child(
                        text(format!("{}{}", r.name, if r.enabled { " · on" } else { "" })).text_size(px(13.0)).text_color(theme::TEXT),
                    );
                    if !r.description.is_empty() {
                        row = row.child(text(r.description.clone()).text_size(px(12.0)).text_color(ink3()));
                    }
                    for line in &r.reads {
                        row = row.child(text(format!("Reads {line}")).text_size(px(12.0)).text_color(ink3()));
                    }
                    if !r.reason.is_empty() {
                        row = row.child(text(r.reason.clone()).text_size(px(12.0)).text_color(warn_ink()));
                    }
                    let mut buttons = view().flex_row().gap(px(8.0));
                    if !r.approved && !r.reads.is_empty() {
                        buttons = buttons.child(sm_btn(Btn::Secondary, || "Approve what it reads".to_owned(), move || {
                            store().v1_routine(&p1, &rid1, crate::mini_v1::host_api::RoutineAction::Approve)
                        }));
                    }
                    if r.enabled {
                        buttons = buttons.child(sm_btn(Btn::Secondary, || "Disable".to_owned(), move || {
                            store().v1_routine(&p2, &rid2, crate::mini_v1::host_api::RoutineAction::Disable)
                        }));
                    } else if r.approved || r.reads.is_empty() {
                        buttons = buttons.child(sm_btn(Btn::Primary, || "Run in this chat".to_owned(), move || {
                            store().v1_routine(&p3, &rid3, crate::mini_v1::host_api::RoutineAction::Enable)
                        }));
                    }
                    routines = routines.child(row.child(buttons));
                }
                page = page.child(routines);
            }
            page.into_element()
        })))
}

/// Add a v1 plugin: its command line, then the review (13.1 step 3) with
/// every line, Cancel (nothing changes) and Install — or, for an update,
/// what it adds and Approve, or Install older version (12.1, 13.2).
fn v1_install_page() -> impl IntoElement {
    view()
        .flex_1()
        .child(back_bar(None, "Add a mini app plugin"))
        .child(scroll().flex_1().child(dynamic(|| {
            let Some(r) = store().v1.review.get() else {
                return pane()
                    .gap(px(10.0))
                    .child(
                        text("The command that starts the plugin (stdio), with its arguments; VAR=value words before it are its environment.")
                            .text_size(px(13.0))
                            .text_color(ink3()),
                    )
                    .child(
                        view().h(px(32.0)).rounded(px(8.0)).bg(tc(0.04)).px(px(10.0)).justify_center().child(
                            input()
                                .value(|| store().v1.form.get())
                                .placeholder("node /path/to/plugin.mjs")
                                .placeholder_color(|| color(tc(0.4)))
                                .text_size(px(13.0))
                                .text_color(theme::TEXT)
                                .h(px(32.0))
                                .on_change(|t: &str| store().v1.form.set(t.to_owned())),
                        ),
                    )
                    .child(
                        view()
                            .flex_row()
                            .gap(px(8.0))
                            .child(sm_btn(Btn::Primary, || "Review…".to_owned(), || store().v1_inspect()))
                            .child(text(|| store().v1.busy.get()).text_size(px(12.0)).text_color(ink3())),
                    )
                    .into_element();
            };
            let action = r.action();
            let mut page = pane()
                .gap(px(10.0))
                .child(text(format!("{} {}", r.title, r.version)).text_size(px(17.0)).font_weight(FontWeight::MEDIUM).text_color(theme::TEXT))
                .child(text(format!("By {}", r.publisher)).text_size(px(13.0)).text_color(ink3()));
            if let Some(from) = &r.from_version {
                page = page.child(
                    text(if r.downgrade {
                        format!("This is an older version than the {from} installed.")
                    } else if *from == r.version {
                        format!("Version {from} is installed already.")
                    } else {
                        format!("Updates {from} to {}.", r.version)
                    })
                    .text_size(px(13.0))
                    .text_color(if r.downgrade { warn_ink() } else { theme::TEXT }),
                );
            }
            if !r.added.is_empty() {
                let mut added = card().child(sec_label("This update adds permissions"));
                for l in &r.added {
                    added = added.child(text(l.clone()).px(px(14.0)).py(px(4.0)).text_size(px(13.0)).text_color(theme::TEXT));
                }
                page = page.child(added);
            }
            let mut lines = card().child(sec_label("What it can do")).pb(px(8.0));
            for l in &r.lines {
                let indent = l.starts_with("  ");
                lines = lines.child(
                    text(l.trim_start().to_owned())
                        .pl(px(if indent { 28.0 } else { 14.0 }))
                        .pr(px(14.0))
                        .py(px(3.0))
                        .text_size(px(13.0))
                        .text_color(if indent { ink2() } else { theme::TEXT }),
                );
            }
            page = page.child(lines);
            for w in &r.warnings {
                page = page.child(text(w.clone()).text_size(px(12.0)).text_color(warn_ink()));
            }
            page.child(
                view()
                    .flex_row()
                    .gap(px(8.0))
                    .child(sm_btn(Btn::Ghost, || "Cancel".to_owned(), || store().v1_cancel_review()))
                    .child(sm_btn(Btn::Primary, move || action.to_owned(), || store().v1_install()))
                    .child(text(|| store().v1.busy.get()).text_size(px(12.0)).text_color(ink3())),
            )
            .into_element()
        })))
}
