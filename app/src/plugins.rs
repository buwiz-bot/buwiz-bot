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

use nsh_core::ui::prelude::*;

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

/// `plugTile` — the connector tile: the brand logo when the icon set grows
/// the brand paths, else the hue letter tile (App.tsx:1460).
fn plug_tile(name: &str, size: f32) -> impl IntoElement {
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
}

/// `chipTile` — the overlapped logo in the "N connected" chip
/// (App.tsx:1468).
fn chip_tile(name: &str, size: f32) -> impl IntoElement {
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
                        .top(px((MK_BAND - 34.0) / 2.0))
                        .right(px(MK_HEAD - 8.0 - 34.0))
                        .size(px(34.0))
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
                        .on_hover_in(|| store().hover_x.set(true))
                        .on_hover_out(|| store().hover_x.set(false))
                        .on_press(|| store().close_plugins())
                        .child(icon("close")),
                )
                .child(
                    when(|| store().plugin_page() == "market", market_page)
                        .otherwise(|| {
                            when(|| store().plugin_page() == "connected", connected_page)
                                .otherwise(|| {
                                    when(|| store().plugin_page() == "plugin", plugin_page)
                                        .otherwise(|| {
                                            when(|| store().plugin_page() == "mcp", mcp_page)
                                                .otherwise(skill_page)
                                        })
                                })
                        })
                        .into_element(),
                ),
        )
}

/// The 48-tall back bar shared by the inner pages (App.tsx:3755):
/// `chev` pages show "‹ Connect Apps"; the rest a bare chevron with the
/// page title centered.
fn back_bar(chev_title: Option<&'static str>, center: &str) -> impl IntoElement {
    let center = center.to_owned();
    view()
        .flex_row()
        .items_center()
        .h(px(48.0))
        .px(px(14.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .w(px(90.0))
                .on_press(|| store().pop_nav())
                .child(icon("chevronBack"))
                .when(chev_title.is_some(), |v| {
                    v.child(
                        text(chev_title.unwrap_or_default())
                            .text_size(px(14.0))
                            .text_color(theme::DIM),
                    )
                }),
        )
        .child(
            view()
                .flex_1()
                .items_center()
                .child(
                    text(center)
                        .number_of_lines(1)
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::TEXT),
                ),
        )
        .child(view().w(px(90.0)))
}

// ═══ MARKET page (App.tsx:3522) ═══

fn market_page() -> impl IntoElement {
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
            scroll().flex_1().child(
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
                    .child(when(
                        || {
                            store().plugin_query.get().is_empty()
                                && store().plugin_cat.get() == "All"
                                && !store().plugin_list().is_empty()
                        },
                        || {
                            view()
                                .child(section_head("Recommended for you", "All", 10.0))
                                .child(grid2(|| {
                                    store().plugin_list().into_iter().take(6).collect()
                                }))
                        },
                    ))
                    .child(each(
                        || {
                            store()
                                .plugin_cats()
                                .into_iter()
                                .filter(|c| {
                                    c != "All"
                                        && (store().plugin_cat.get() == "All"
                                            || store().plugin_cat.get() == *c)
                                })
                                .collect::<Vec<_>>()
                        },
                        |c| c.clone(),
                        |c, _| cat_section(c.get()),
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
                .on_hover_in(|| store().hover_tpl.set(true))
                .on_hover_out(|| store().hover_tpl.set(false))
                .child(
                    crate::avatar::bot_blob(crate::avatar::BotBlobProps {
                        shape: "blob".into(),
                        color: "#f5f5f5".into(),
                        size: 20.0,
                        paused: true,
                        ..Default::default()
                    }),
                )
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
        .on_hover_in(|| store().hover_chip.set(true))
        .on_hover_out(|| store().hover_chip.set(false))
        .on_press(|| store().push_nav("connected", "", ""))
        .child(
            view()
                .flex_row()
                .pl(px(2.0))
                .child(each(
                    || {
                        let st = store();
                        let (inst, standalone) = st.conn_items();
                        inst.iter()
                            .map(|p| p.name.clone())
                            .chain(standalone.iter().map(|s| {
                                if s.name.is_empty() {
                                    s.id.clone()
                                } else {
                                    s.name.clone()
                                }
                            }))
                            .take(4)
                            .collect::<Vec<_>>()
                    },
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
                let (inst, standalone) = st.conn_items();
                format!(
                    "{} Connected / {} Skills",
                    inst.len() + standalone.len(),
                    st.private_skills.get().len()
                )
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
                .text_color(move || {
                    if store().plugin_cat.get() == c4 {
                        hx("#121212")
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
    let cat2 = cat.clone();
    view()
        .child(section_head_owned(&label, 22.0))
        .child(grid2(move || {
            store()
                .plugin_list()
                .into_iter()
                .filter(|p| p.category.eq_ignore_ascii_case(&cat))
                .collect()
        }))
        .child(when(
            move || {
                store()
                    .plugin_list()
                    .iter()
                    .any(|p| p.category.eq_ignore_ascii_case(&cat2))
            },
            || view().h(px(0.0)),
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
        .on_hover_in(move || store().hover_plugin.set(n_hin.clone()))
        .on_hover_out(move || {
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
                .min_w(px(0.0))
                .max_w(px(200.0))
                .child(
                    text(Store::plugin_title(&p.name))
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
                .on_hover_in(move || store().hover_add.set(n_ain.clone()))
                .on_hover_out(move || {
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
                            .on_hover_in(move || store().drop_sel.set(i.get()))
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
                                text(Store::plugin_title(&p.get().name))
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
    view()
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
                .on_hover_in(move || store().hover_inst.set(key_in.clone()))
                .on_hover_out(move || {
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
                .on_hover_in(move || store().hover_inst.set(key_in.clone()))
                .on_hover_out(move || {
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

// ═══ PLUGIN detail page (App.tsx:3861) ═══

fn plugin_page() -> impl IntoElement {
    let name = store()
        .plugin_nav
        .get_untracked()
        .last()
        .map(|n| n.name.clone())
        .unwrap_or_default();
    let n_acc = name.clone();
    let n_acc2 = name.clone();
    let n_apps = name.clone();
    let n_apps2 = name.clone();
    let n_tools = name.clone();
    let n_skills = name.clone();
    let n_skills2 = name.clone();
    let n_cmds = name.clone();
    let n_cmds2 = name.clone();
    let n_desc = name.clone();
    view()
        .child(back_bar(None, ""))
        .child(
            scroll().flex_1().child(
                view()
                    .px(px(21.0))
                    .pb(px(28.0))
                    // header: icon + name + View Source ↗ ··· Add/Uninstall
                    .child(
                        view()
                            .flex_row()
                            .items_start()
                            .gap(px(14.0))
                            .pb(px(14.0))
                            .child(plug_tile(&name, 56.0))
                            .child(
                                view()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .child(
                                        text(Store::plugin_title(&name))
                                            .text_size(px(16.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme::TEXT),
                                    )
                                    .child(
                                        text("View Source ↗")
                                            .text_size(px(12.0))
                                            .text_color(tc(0.56))
                                            .pt(px(2.0)),
                                    ),
                            )
                            .child(install_btn(name.clone())),
                    )
                    // description — detail's, else the catalog row's.
                    .child(
                        text(move || {
                            let st = store();
                            let dd = st.plugin_details.get().get(&n_desc).cloned();
                            dd.and_then(|d| {
                                if d.description.is_empty() {
                                    None
                                } else {
                                    Some(d.description)
                                }
                            })
                            .or_else(|| {
                                st.plugins
                                    .get()
                                    .iter()
                                    .find(|p| p.name == n_desc)
                                    .map(|p| p.description.clone())
                            })
                            .unwrap_or_default()
                        })
                        .text_size(px(13.0))
                        .text_color(tc(0.8))
                        .pb_4(),
                    )
                    // Accounts — one row per connector.
                    .child(when(
                        move || {
                            store()
                                .plugin_details
                                .get()
                                .get(&n_acc)
                                .map(|d| !d.mcp.is_empty())
                                .unwrap_or(false)
                        },
                        move || accounts_card(n_acc2.clone()),
                    ))
                    // Apps (connectors)
                    .child(when(
                        move || {
                            store()
                                .plugin_details
                                .get()
                                .get(&n_apps)
                                .map(|d| !d.mcp.is_empty())
                                .unwrap_or(false)
                        },
                        move || apps_card(n_apps2.clone()),
                    ))
                    // Tools — collapsible per connector.
                    .child(each(
                        move || {
                            store()
                                .plugin_details
                                .get()
                                .get(&n_tools)
                                .map(|d| {
                                    d.mcp
                                        .iter()
                                        .filter(|m| !m.id.is_empty())
                                        .map(|m| m.id.clone())
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default()
                        },
                        |id| id.clone(),
                        |id, _| mcp_tools_section(id.get()),
                    ))
                    // Skills.
                    .child(when(
                        move || {
                            store()
                                .plugin_details
                                .get()
                                .get(&n_skills)
                                .map(|d| !d.skills.is_empty())
                                .unwrap_or(false)
                        },
                        move || skills_card(n_skills2.clone()),
                    ))
                    // Commands.
                    .child(when(
                        move || {
                            store()
                                .plugin_details
                                .get()
                                .get(&n_cmds)
                                .map(|d| !d.commands.is_empty())
                                .unwrap_or(false)
                        },
                        move || commands_card(n_cmds2.clone()),
                    ))
                    // Information.
                    .child(info_card(&name)),
            ),
        )
}

/// Add / Uninstall pill — `installed` read live off the detail.
fn install_btn(name: String) -> impl IntoElement {
    let n = name.clone();
    let n_un = name.clone();
    let n_un2 = name.clone();
    let n_in = name.clone();
    let n_in2 = name.clone();
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
            let n_un2 = n_un2.clone();
            view()
                .h(px(32.0))
                .rounded(px(999.0))
                .bg(tc(0.07))
                .px(px(14.0))
                .items_center()
                .justify_center()
                .on_press(move || store().uninstall_plugin(&n_un))
                .child(
                    text(move || {
                        if store().plugin_busy.get() == n_un2 {
                            "…".to_owned()
                        } else {
                            "Uninstall".to_owned()
                        }
                    })
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::TEXT),
                )
        },
    )
    .otherwise(move || {
        let n_in = n_in.clone();
        let n_in2 = n_in2.clone();
        view()
            .h(px(32.0))
            .rounded(px(999.0))
            .bg(tc(0.96))
            .px_4()
            .items_center()
            .justify_center()
            .on_press(move || store().install_plugin(&n_in))
            .child(
                text(move || {
                    if store().plugin_busy.get() == n_in2 {
                        "Adding…".to_owned()
                    } else {
                        "Add".to_owned()
                    }
                })
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(hx("#1f1f1f")),
            )
    })
}

fn accounts_card(name: String) -> impl IntoElement {
    let n = name.clone();
    let n2 = name.clone();
    view()
        .child(
            text("Accounts")
                .text_size(px(12.0))
                .text_color(theme::DIM)
                .pl(px(2.0))
                .pb_2(),
        )
        .child(
            view()
                .rounded(px(12.0))
                .bg(tc(0.04))
                .px(px(14.0))
                .child(each(
                    move || {
                        store()
                            .plugin_details
                            .get()
                            .get(&n)
                            .map(|d| d.mcp.clone())
                            .unwrap_or_default()
                    },
                    |m| m.id.clone() + &m.name,
                    move |m, i| account_row(m.get(), n2.clone(), i.get() > 0),
                ))
                .child(
                    view()
                        .border_t(px(0.5))
                        .border_t_color(tc(0.07))
                        .py_3()
                        .child(
                            text("＋ Add Another Account")
                                .text_size(px(13.0))
                                .text_color(theme::DIM),
                        ),
                ),
        )
}

fn account_row(
    m: crate::store::PluginMcp,
    plugin: String,
    sep: bool,
) -> impl IntoElement {
    let status = m.status.clone();
    let status2 = m.status.clone();
    let err = m.err.clone();
    let err2 = m.err.clone();
    view()
        .border_t(if sep { px(0.5) } else { px(0.0) })
        .border_t_color(tc(0.07))
        .py_3()
        .child(
            view()
                .flex_row()
                .items_center()
                .child(
                    text("default ⌘")
                        .number_of_lines(1)
                        .flex_1()
                        .text_size(px(13.0))
                        .text_color(theme::TEXT),
                )
                .child(match status.as_str() {
                    "connected" => text("Connected")
                        .text_size(px(12.0))
                        .text_color(hx("#32bb78"))
                        .into_element(),
                    "error" => {
                        let needs = Store::needs_auth(&err);
                        view()
                            .flex_row()
                            .items_center()
                            .child(
                                text(if needs { "Needs auth" } else { "Error" })
                                    .text_size(px(12.0))
                                    .text_color(if needs {
                                        hx("#ffb224")
                                    } else {
                                        hx("#ff5f57")
                                    })
                                    .pr_3(),
                            )
                            .child(
                                view()
                                    .h(px(26.0))
                                    .rounded(px(999.0))
                                    .bg(tc(0.07))
                                    .px_3()
                                    .items_center()
                                    .justify_center()
                                    .on_press(move || {
                                        store().auth_or_retry(
                                            &m.id,
                                            &m.transport,
                                            &m.err,
                                            &plugin,
                                        )
                                    })
                                    .child(
                                        text("Retry")
                                            .text_size(px(12.0))
                                            .text_color(theme::TEXT),
                                    ),
                            )
                            .into_element()
                    }
                    _ => text(if status.is_empty() {
                        "Idle".to_owned()
                    } else {
                        status.clone()
                    })
                    .text_size(px(12.0))
                    .text_color(theme::DIM)
                    .into_element(),
                }),
        )
        .when(status2 == "error" && !Store::needs_auth(&err2) && !err2.is_empty(), |v| {
            v.child(
                text(format!("Failed to load connector. Details: {}", err2))
                    .number_of_lines(3)
                    .text_size(px(11.0))
                    .text_color(theme::DIM)
                    .pt_1(),
            )
        })
}

fn apps_card(name: String) -> impl IntoElement {
    let n = name.clone();
    view()
        .child(
            text(move || {
                let n = store()
                    .plugin_details
                    .get()
                    .get(&name)
                    .map(|d| d.mcp.len())
                    .unwrap_or(0);
                format!("Apps  {n}")
            })
            .text_size(px(12.0))
            .text_color(theme::DIM)
            .pl(px(2.0))
            .pt_4()
            .pb_2(),
        )
        .child(
            view().rounded(px(12.0)).bg(tc(0.04)).child(each(
                move || {
                    store()
                        .plugin_details
                        .get()
                        .get(&n)
                        .map(|d| d.mcp.clone())
                        .unwrap_or_default()
                },
                |m| m.id.clone() + &m.name,
                |m, _| {
                    let m = m.get();
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(10.0))
                        .px(px(14.0))
                        .py(px(11.0))
                        .child(text("🔌").text_size(px(13.0)).text_color(theme::DIM))
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(0.0))
                                .child(
                                    text(m.name.clone())
                                        .number_of_lines(1)
                                        .text_size(px(13.0))
                                        .text_color(theme::TEXT),
                                )
                                .child(
                                    text(format!(
                                        "Connector{}",
                                        if m.transport == "remote" { " · remote" } else { "" }
                                    ))
                                    .number_of_lines(1)
                                    .text_size(px(11.0))
                                    .text_color(theme::DIM),
                                ),
                        )
                },
            )),
        )
}

/// `toolSwitch` — Grok's blue pill toggle (App.tsx:1554).
fn tool_switch(
    on: impl Fn() -> bool + 'static,
    on_toggle: impl Fn() + 'static,
) -> impl IntoElement {
    let on2 = std::rc::Rc::new(on);
    let on3 = on2.clone();
    view()
        .w(px(40.0))
        .h(px(24.0))
        .rounded(px(12.0))
        .justify_center()
        .bg(move || {
            if (on2)() {
                hx("#0a84ff")
            } else {
                tc(0.12)
            }
        })
        .on_press(move || on_toggle())
        .child(
            view()
                .size(px(20.0))
                .rounded(px(10.0))
                .bg(Hsla::from_str("#ffffff").unwrap_or(theme::ACCENT.dark))
                .ml(move || {
                    if (on3)() {
                        px(16.0)
                    } else {
                        px(0.0)
                    }
                }),
        )
}

/// `mcpToolsSection` — "N of M enabled" collapsible (App.tsx:1563).
fn mcp_tools_section(server_id: String) -> impl IntoElement {
    let id = server_id.clone();
    let id2 = server_id.clone();
    let id3 = server_id.clone();
    let id4 = server_id.clone();
    let id5 = server_id;
    view()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .pt_4()
                .pb_2()
                .on_press(move || {
                    let open = store()
                        .tools_open
                        .get_untracked()
                        .get(&id)
                        .copied()
                        .unwrap_or(false);
                    let id2 = id.clone();
                    store().tools_open.update(|m| {
                        m.insert(id2, !open);
                    });
                })
                .child(
                    text("Tools")
                        .text_size(px(12.0))
                        .text_color(theme::DIM)
                        .pl(px(2.0)),
                )
                .child(
                    text(move || {
                        let st = store();
                        let tools = st
                            .mcp_tools
                            .get()
                            .get(&id2)
                            .cloned()
                            .unwrap_or_default();
                        let on = tools.iter().filter(|t| t.enabled).count();
                        let open = st
                            .tools_open
                            .get()
                            .get(&id2)
                            .copied()
                            .unwrap_or(false);
                        format!(
                            "{} of {} enabled {}",
                            on,
                            tools.len(),
                            if open { "⌃" } else { "⌄" }
                        )
                    })
                    .text_size(px(12.0))
                    .text_color(theme::DIM),
                ),
        )
        .child(when(
            move || {
                store()
                    .tools_open
                    .get()
                    .get(&id3)
                    .copied()
                    .unwrap_or(false)
            },
            move || {
                let id4 = id4.clone();
                let id5 = id5.clone();
                view().rounded(px(12.0)).bg(tc(0.04)).child(each(
                    move || {
                        store()
                            .mcp_tools
                            .get()
                            .get(&id4)
                            .cloned()
                            .unwrap_or_default()
                    },
                    |t| t.name.clone(),
                    move |t, _| {
                        let sid = id5.clone();
                        let name = t.get().name.clone();
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(10.0))
                            .px(px(14.0))
                            .py(px(11.0))
                            .child(icon("tool"))
                            .child(
                                text(move || t.get().name)
                                    .number_of_lines(1)
                                    .flex_1()
                                    .text_size(px(13.0))
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
        ))
}

fn skills_card(name: String) -> impl IntoElement {
    let n = name.clone();
    let n2 = name.clone();
    let n3 = name.clone();
    let n4 = name.clone();
    let n_head = name;
    view()
        .child(
            text(move || {
                let n = store()
                    .plugin_details
                    .get()
                    .get(&n_head)
                    .map(|d| d.skills.len())
                    .unwrap_or(0);
                format!("Skills  {n}")
            })
            .text_size(px(12.0))
            .text_color(theme::DIM)
            .pl(px(2.0))
            .pt_4()
            .pb_2(),
        )
        .child(
            view().rounded(px(12.0)).bg(tc(0.04)).child(each(
                move || {
                    store()
                        .plugin_details
                        .get()
                        .get(&n)
                        .map(|d| {
                            let list = d.skills.clone();
                            if store().skills_expanded.get() {
                                list
                            } else {
                                list.into_iter().take(5).collect()
                            }
                        })
                        .unwrap_or_default()
                },
                |s| s.name.clone(),
                move |s, _| {
                    let sk = s.get();
                    let pname = n3.clone();
                    let skn = sk.name.clone();
                    view()
                        .flex_row()
                        .gap(px(10.0))
                        .px(px(14.0))
                        .py(px(11.0))
                        .items_start()
                        .on_press(move || {
                            store().push_nav("skill", &pname, &skn)
                        })
                        .child(
                            text("📦")
                                .text_size(px(14.0))
                                .text_color(theme::DIM)
                                .pt(px(1.0)),
                        )
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(0.0))
                                .child(
                                    text(sk.name.clone())
                                        .number_of_lines(1)
                                        .text_size(px(13.0))
                                        .text_color(theme::TEXT),
                                )
                                .child(
                                    text(sk.description.clone())
                                        .number_of_lines(2)
                                        .text_size(px(11.0))
                                        .text_color(theme::DIM),
                                ),
                        )
                },
            )),
        )
        .child(when(
            move || {
                store()
                    .plugin_details
                    .get()
                    .get(&n2)
                    .map(|d| d.skills.len() > 5)
                    .unwrap_or(false)
            },
            move || {
                let n4 = n4.clone();
                text(move || {
                    let n = store()
                        .plugin_details
                        .get()
                        .get(&n4)
                        .map(|d| d.skills.len())
                        .unwrap_or(0);
                    if store().skills_expanded.get() {
                        "Show less".to_owned()
                    } else {
                        format!("{} more", n.saturating_sub(5))
                    }
                })
                .text_size(px(12.0))
                .text_color(theme::DIM)
                .pt(px(10.0))
                .self_center()
                .on_press(|| store().skills_expanded.update(|v| *v = !*v))
            },
        ))
}

fn commands_card(name: String) -> impl IntoElement {
    let n = name.clone();
    view()
        .child(
            text(move || {
                let n = store()
                    .plugin_details
                    .get()
                    .get(&name)
                    .map(|d| d.commands.len())
                    .unwrap_or(0);
                format!("Commands  {n}")
            })
            .text_size(px(12.0))
            .text_color(theme::DIM)
            .pl(px(2.0))
            .pt_4()
            .pb_2(),
        )
        .child(
            view().rounded(px(12.0)).bg(tc(0.04)).child(each(
                move || {
                    store()
                        .plugin_details
                        .get()
                        .get(&n)
                        .map(|d| d.commands.clone())
                        .unwrap_or_default()
                },
                |c| c.name.clone(),
                |c, _| {
                    view()
                        .flex_row()
                        .gap(px(10.0))
                        .px(px(14.0))
                        .py(px(10.0))
                        .child(icon("command"))
                        .child(
                            text(c.get().name)
                                .number_of_lines(1)
                                .text_size(px(13.0))
                                .text_color(theme::TEXT),
                        )
                },
            )),
        )
}

fn info_card(name: &str) -> impl IntoElement {
    let n_cap = name.to_owned();
    let n_dev = name.to_owned();
    let n_cat = name.to_owned();
    let n_web = name.to_owned();
    view()
        .child(
            text("Information")
                .text_size(px(12.0))
                .text_color(theme::DIM)
                .pl(px(2.0))
                .pt_4()
                .pb_2(),
        )
        .child(
            view()
                .rounded(px(12.0))
                .bg(tc(0.04))
                .px(px(14.0))
                .child(info_row(
                    "Capabilities",
                    move || {
                        let d = store()
                            .plugin_details
                            .get()
                            .get(&n_cap)
                            .cloned()
                            .unwrap_or_default();
                        format!(
                            "{} app{}, {} skill{}",
                            d.mcp.len(),
                            if d.mcp.len() == 1 { "" } else { "s" },
                            d.skills.len(),
                            if d.skills.len() == 1 { "" } else { "s" },
                        )
                    },
                    false,
                ))
                .child(info_row(
                    "Developer",
                    move || {
                        let d = store()
                            .plugin_details
                            .get()
                            .get(&n_dev)
                            .cloned()
                            .unwrap_or_default();
                        if !d.author.is_empty() {
                            d.author
                        } else {
                            host_label(&d.homepage)
                        }
                    },
                    true,
                ))
                .child(info_row(
                    "Category",
                    move || {
                        let d = store()
                            .plugin_details
                            .get()
                            .get(&n_cat)
                            .cloned()
                            .unwrap_or_default();
                        Store::plugin_title(&d.category)
                    },
                    true,
                ))
                .child(info_row(
                    "Website",
                    move || {
                        let d = store()
                            .plugin_details
                            .get()
                            .get(&n_web)
                            .cloned()
                            .unwrap_or_default();
                        host(&d.homepage)
                    },
                    true,
                ))
                .child(info_row("Availability", || "Public".into(), true)),
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

/// `host.split(".")[0]` — the Developer fallback (App.tsx:3999).
fn host_label(url: &str) -> String {
    host(url)
        .split('.')
        .next()
        .map(|s| s.to_owned())
        .unwrap_or_else(|| "—".to_owned())
}

fn info_row(
    label: &'static str,
    value: impl Fn() -> String + 'static,
    sep: bool,
) -> impl IntoElement {
    view()
        .flex_row()
        .justify_between()
        .py_3()
        .border_t(if sep { px(0.5) } else { px(0.0) })
        .border_t_color(tc(0.07))
        .child(text(label).text_size(px(13.0)).text_color(theme::TEXT))
        .child(text(value).text_size(px(13.0)).text_color(theme::DIM))
}

// ═══ MCP server detail page (App.tsx:4020) ═══

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
        return view().child(back_bar(None, "")).into_element();
    };
    let title = if s.name.is_empty() {
        s.id.clone()
    } else {
        s.name.clone()
    };
    let needs = Store::needs_auth(&s.err);
    let transport = if s.url.is_empty() { "command" } else { "remote" }.to_owned();
    let url_or_cmd = if s.url.is_empty() {
        format!("{} {}", s.command, s.args.join(" ")).trim().to_owned()
    } else {
        s.url.clone()
    };
    let url_label = if s.url.is_empty() { "Command" } else { "URL" };
    let sid = s.id.clone();
    let sid2 = s.id.clone();
    let sid3 = s.id.clone();
    let serr = s.err.clone();
    let status = s.status.clone();
    let err_txt = format!("Failed to load connector. Details: {}", s.err);
    view()
        .child(back_bar(None, &title))
        .child(
            scroll().flex_1().child(
                view()
                    .px(px(21.0))
                    .pb(px(28.0))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(14.0))
                            .pb_4()
                            .child(plug_tile(&title, 56.0))
                            .child(
                                text(title.clone())
                                    .flex_1()
                                    .text_size(px(16.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::TEXT),
                            )
                            .child(
                                view()
                                    .h(px(32.0))
                                    .rounded(px(999.0))
                                    .bg(tc(0.07))
                                    .px(px(14.0))
                                    .items_center()
                                    .justify_center()
                                    .on_press(move || {
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
                                    })
                                    .child(
                                        text("Uninstall")
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme::TEXT),
                                    ),
                            ),
                    )
                    .child(
                        text("Accounts")
                            .text_size(px(12.0))
                            .text_color(theme::DIM)
                            .pl(px(2.0))
                            .pb_2(),
                    )
                    .child(
                        view()
                            .rounded(px(12.0))
                            .bg(tc(0.04))
                            .px(px(14.0))
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .py_3()
                                    .child(
                                        text("default")
                                            .text_size(px(13.0))
                                            .text_color(theme::TEXT),
                                    )
                                    .child(view().flex_1())
                                    .child(match status.as_str() {
                                        "connected" => text("Connected")
                                            .text_size(px(12.0))
                                            .text_color(hx("#32bb78"))
                                            .into_element(),
                                        "error" => view()
                                            .flex_row()
                                            .items_center()
                                            .child(
                                                text(if needs { "Needs auth" } else { "Error" })
                                                    .text_size(px(12.0))
                                                    .text_color(if needs {
                                                        hx("#ffb224")
                                                    } else {
                                                        hx("#ff5f57")
                                                    })
                                                    .pr_3(),
                                            )
                                            .child(
                                                view()
                                                    .h(px(26.0))
                                                    .rounded(px(999.0))
                                                    .bg(tc(0.07))
                                                    .px_3()
                                                    .items_center()
                                                    .justify_center()
                                                    .on_press(move || {
                                                        store().auth_or_retry(
                                                            &sid2,
                                                            &transport,
                                                            &serr,
                                                            "",
                                                        )
                                                    })
                                                    .child(
                                                        text("Retry")
                                                            .text_size(px(12.0))
                                                            .text_color(theme::TEXT),
                                                    ),
                                            )
                                            .into_element(),
                                        _ => text(if s.enabled { "Idle" } else { "Off" })
                                            .text_size(px(12.0))
                                            .text_color(theme::DIM)
                                            .into_element(),
                                    }),
                            )
                            .when(
                                status == "error" && !needs && !s.err.is_empty(),
                                |v| {
                                    v.child(
                                        text(err_txt)
                                            .number_of_lines(3)
                                            .text_size(px(11.0))
                                            .text_color(theme::DIM)
                                            .pb(px(10.0)),
                                    )
                                },
                            ),
                    )
                    .child(mcp_tools_section(sid3.clone()))
                    .child(
                        text("Details")
                            .text_size(px(12.0))
                            .text_color(theme::DIM)
                            .pl(px(2.0))
                            .pt_4()
                            .pb_2(),
                    )
                    .child(
                        view()
                            .rounded(px(12.0))
                            .bg(tc(0.04))
                            .px(px(14.0))
                            .child(info_row("Source", || "Added manually".into(), false))
                            .child(info_row(
                                "Transport",
                                move || {
                                    if s.url.is_empty() {
                                        "Command".to_owned()
                                    } else {
                                        "Remote".to_owned()
                                    }
                                },
                                true,
                            ))
                            .child(
                                view()
                                    .flex_row()
                                    .justify_between()
                                    .gap(px(20.0))
                                    .py_3()
                                    .border_t(px(0.5))
                                    .border_t_color(tc(0.07))
                                    .child(
                                        text(url_label)
                                            .text_size(px(13.0))
                                            .text_color(theme::TEXT),
                                    )
                                    .child(
                                        text(url_or_cmd)
                                            .number_of_lines(2)
                                            .flex_1()
                                            .text_size(px(13.0))
                                            .text_color(theme::DIM),
                                    ),
                            )
                            .child(info_row(
                                "Tools",
                                move || format!("{}", s.tools),
                                true,
                            )),
                    ),
            ),
        )
        .into_element()
}

// ═══ SKILL page (App.tsx:4096) ═══

fn skill_page() -> impl IntoElement {
    let nav = store()
        .plugin_nav
        .get_untracked()
        .last()
        .cloned()
        .unwrap_or_else(PluginNav::market);
    let title = if nav.id.is_empty() {
        "New Skill".to_owned()
    } else {
        nav.id.clone()
    };
    view()
        .child(back_bar(None, &title))
        .child(if !nav.name.is_empty() {
            plugin_skill_view(&nav).into_element()
        } else {
            private_skill_editor().into_element()
        })
}

/// The plugin skill read-only viewer (App.tsx:4108).
fn plugin_skill_view(nav: &PluginNav) -> impl IntoElement {
    let key = format!("{}/{}", nav.name, nav.id);
    let key2 = key.clone();
    let key3 = key.clone();
    scroll()
        .flex_1()
        .child(
            view()
                .px(px(21.0))
                .pb(px(28.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(14.0))
                        .pb(px(14.0))
                        .child(
                            view()
                                .size(px(48.0))
                                .rounded(px(12.0))
                                .bg(tc(0.07))
                                .items_center()
                                .justify_center()
                                .child(
                                    text("📦")
                                        .text_size(px(18.0))
                                        .text_color(theme::DIM),
                                ),
                        )
                        .child(
                            view()
                                .child(
                                    text(nav.id.clone())
                                        .text_size(px(15.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme::TEXT),
                                )
                                .child(
                                    text(format!("Plugin skill · {}", nav.name))
                                        .text_size(px(12.0))
                                        .text_color(theme::DIM)
                                        .pt(px(2.0)),
                                ),
                        ),
                )
                .child(
                    text(move || {
                        store()
                            .skill_bodies
                            .get()
                            .get(&key)
                            .map(|b| b.description.clone())
                            .unwrap_or_default()
                    })
                    .text_size(px(13.0))
                    .text_color(tc(0.8))
                    .pb(px(14.0)),
                )
                .child(
                    text("Name")
                        .text_size(px(12.0))
                        .text_color(theme::DIM)
                        .pl(px(2.0))
                        .pb(px(6.0)),
                )
                .child(
                    view()
                        .rounded(px(10.0))
                        .bg(tc(0.04))
                        .px_3()
                        .py(px(10.0))
                        .child(
                            text(nav.id.clone())
                                .text_size(px(13.0))
                                .text_color(theme::TEXT),
                        ),
                )
                .child(
                    text("Description")
                        .text_size(px(12.0))
                        .text_color(theme::DIM)
                        .pl(px(2.0))
                        .pt_3()
                        .pb(px(6.0)),
                )
                .child(
                    view()
                        .rounded(px(10.0))
                        .bg(tc(0.04))
                        .px_3()
                        .py(px(10.0))
                        .child(
                            text(move || {
                                store()
                                    .skill_bodies
                                    .get()
                                    .get(&key2)
                                    .map(|b| {
                                        if b.description.is_empty() {
                                            "—".to_owned()
                                        } else {
                                            b.description.clone()
                                        }
                                    })
                                    .unwrap_or_else(|| "—".to_owned())
                            })
                            .text_size(px(13.0))
                            .text_color(theme::TEXT),
                        ),
                )
                .child(
                    text("Instructions")
                        .text_size(px(12.0))
                        .text_color(theme::DIM)
                        .pl(px(2.0))
                        .pt_3()
                        .pb(px(6.0)),
                )
                .child(
                    view()
                        .rounded(px(10.0))
                        .bg(tc(0.04))
                        .px_3()
                        .py(px(10.0))
                        .min_h(px(140.0))
                        .child(
                            text(move || {
                                store()
                                    .skill_bodies
                                    .get()
                                    .get(&key3)
                                    .map(|b| b.body.clone())
                                    .unwrap_or_else(|| "Loading…".to_owned())
                            })
                            .text_size(px(12.0))
                            .text_color(theme::TEXT),
                        ),
                ),
        )
}

/// The private skill editor — name/description/instructions + Save +
/// Delete Skill (App.tsx:4139).
fn private_skill_editor() -> impl IntoElement {
    view().flex_1().child(
        scroll().flex_1().child(
            view()
                .px(px(21.0))
                .pb(px(28.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(14.0))
                        .pb(px(14.0))
                        .child(
                            view()
                                .size(px(48.0))
                                .rounded(px(12.0))
                                .bg(tc(0.07))
                                .items_center()
                                .justify_center()
                                .child(
                                    text("📦")
                                        .text_size(px(18.0))
                                        .text_color(theme::DIM),
                                ),
                        )
                        .child(
                            view()
                                .flex_1()
                                .child(
                                    text(move || {
                                        let n = store().ps_name.get();
                                        if n.is_empty() {
                                            "New Skill".to_owned()
                                        } else {
                                            n
                                        }
                                    })
                                    .text_size(px(15.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::TEXT),
                                )
                                .child(
                                    text("Private skill")
                                        .text_size(px(12.0))
                                        .text_color(theme::DIM)
                                        .pt(px(2.0)),
                                ),
                        )
                        .child(when(
                            || !store().ps_id.get().is_empty(),
                            || {
                                view()
                                    .h(px(34.0))
                                    .rounded(px(999.0))
                                    .border_1()
                                    .border_color(hx("#ff5f5740"))
                                    .px_4()
                                    .items_center()
                                    .justify_center()
                                    .on_press(|| store().delete_private_skill())
                                    .child(
                                        text("Delete Skill")
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(hx("#ff5f57")),
                                    )
                            },
                        )),
                )
                .child(
                    text("Name")
                        .text_size(px(12.0))
                        .text_color(theme::DIM)
                        .pl(px(2.0))
                        .pb(px(6.0)),
                )
                .child(
                    view()
                        .rounded(px(10.0))
                        .bg(tc(0.04))
                        .px_3()
                        .child(
                            input()
                                .value(move || store().ps_name.get())
                                .placeholder("skill-name")
                                .placeholder_color(|| color(tc(0.3)))
                                .text_size(px(13.0))
                                .text_color(theme::TEXT)
                                .py(px(10.0))
                                .on_change(|t: &str| store().ps_name.set(t.to_owned())),
                        ),
                )
                .child(
                    text("Description")
                        .text_size(px(12.0))
                        .text_color(theme::DIM)
                        .pl(px(2.0))
                        .pt_3()
                        .pb(px(6.0)),
                )
                .child(
                    view()
                        .rounded(px(10.0))
                        .bg(tc(0.04))
                        .px_3()
                        .child(
                            input()
                                .value(move || store().ps_desc.get())
                                .placeholder("When to use this skill")
                                .placeholder_color(|| color(tc(0.3)))
                                .text_size(px(13.0))
                                .text_color(theme::TEXT)
                                .py(px(10.0))
                                .on_change(|t: &str| store().ps_desc.set(t.to_owned())),
                        ),
                )
                .child(
                    text("Instructions")
                        .text_size(px(12.0))
                        .text_color(theme::DIM)
                        .pl(px(2.0))
                        .pt_3()
                        .pb(px(6.0)),
                )
                .child(
                    view()
                        .rounded(px(10.0))
                        .bg(tc(0.04))
                        .px_3()
                        .min_h(px(160.0))
                        .child(
                            input()
                                .value(move || store().ps_body.get())
                                .placeholder("What the agent should do, in markdown")
                                .placeholder_color(|| color(tc(0.3)))
                                .multiline(true)
                                .text_size(px(12.0))
                                .text_color(theme::TEXT)
                                .h(px(160.0))
                                .py(px(10.0))
                                .on_change(|t: &str| store().ps_body.set(t.to_owned())),
                        ),
                )
                .child(
                    view()
                        .flex_row()
                        .justify_end()
                        .pt(px(14.0))
                        .child(
                            view()
                                .h(px(34.0))
                                .rounded(px(999.0))
                                .px(px(20.0))
                                .items_center()
                                .justify_center()
                                .bg(move || {
                                    if !store().ps_name.get().trim().is_empty()
                                        && !store().ps_body.get().trim().is_empty()
                                    {
                                        tc(0.96)
                                    } else {
                                        tc(0.12)
                                    }
                                })
                                .on_press(|| store().save_private_skill())
                                .child(
                                    text("Save")
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(move || {
                                            if !store().ps_name.get().trim().is_empty()
                                                && !store().ps_body.get().trim().is_empty()
                                            {
                                                hx("#1f1f1f")
                                            } else {
                                                theme::DIM
                                            }
                                        }),
                                ),
                        ),
                ),
        ),
    )
}
