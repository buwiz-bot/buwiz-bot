//! The work tab's "Mini Apps" shelf — a port of `widgets/index.tsx`'s
//! `WidgetShelf`: the `/catalog` grid with filter chips, double-click
//! renames, tag editing, the "＋ chat" reuse button and each card's
//! context menu (the TSX's `Menu.popup` items ride the shell's ctx card).

use std::str::FromStr;

use nsh_core::ui::prelude::*;

use crate::{ctx, live::store, store::ShelfItem};

/// `KIND_COLOR` — the kinds' badge colors (widgets/index.tsx:28).
fn kind_color(k: &str) -> AdaptiveColor {
    match k {
        "db" => ac("#10a37f"),
        "api" => ac("#5e6ad2"),
        "keys" => ac("#f2994a"),
        _ => ac("#f5f5f585"),
    }
}

/// hex → `AdaptiveColor` (the shelf is dark-only in the TSX).
fn ac(hex: &str) -> AdaptiveColor {
    AdaptiveColor::from(Hsla::from_str(hex).unwrap_or(Hsla {
        h: 0.0,
        s: 0.0,
        l: 1.0,
        a: 1.0,
    }))
}

/// The `act` glyph button (`↗`, `⋯`) — 12px text on transparent.
fn act_btn(g: &str, on_press: impl Fn() + 'static) -> View {
    view()
        .rounded(px(7.0))
        .px(px(8.0))
        .py(px(4.0))
        .on_press(move || on_press())
        .child(
            text(g.to_owned())
                .text_size(px(12.0))
                .text_color(ac("#f5f5f585")),
        )
}

/// The `⋯` menu button — opens the card's menu anchored under itself
/// (the TSX's `Menu.popup` positions off the measured button rect).
fn menu_btn(items: Vec<crate::store::CtxItem>) -> View {
    let node = element_ref();
    view()
        .element_ref(&node)
        .rounded(px(7.0))
        .px(px(8.0))
        .py(px(4.0))
        .on_press(move || {
            let items = items.clone();
            spawn(async move {
                if let Some(m) = node.measure().await {
                    ctx::open_at(m.page.x.0, m.page.y.0 + m.frame.size.height.0, items);
                }
            });
        })
        .child(
            text("⋯".to_owned())
                .text_size(px(12.0))
                .text_color(ac("#f5f5f585")),
        )
}

/// A filter chip (`chip(label, on, onTap, color)`).
fn chip(
    label: String,
    on: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
    color: &str,
) -> View {
    let color = ac(color);
    view()
        .rounded(px(999.0))
        .px(px(10.0))
        .py(px(3.0))
        .on_press(move || on_press())
        .bg(move || if on() { color } else { ac("#f5f5f50f") })
        .child(
            text(label)
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ac("#f5f5f5")),
        )
}

/// A kind filter chip (`db`/`api`/`keys`/`mem`) — a press toggles it, a
/// second press returns to `all`.
fn kind_chip(k: &str) -> View {
    let k = k.to_owned();
    let k2 = k.clone();
    let k3 = k.clone();
    let color = match k.as_str() {
        "db" => "#10a37f",
        "api" => "#5e6ad2",
        "keys" => "#f2994a",
        _ => "#f5f5f585",
    };
    chip(
        k3,
        move || store().shelf_kind.get() == k,
        move || {
            let st = store();
            st.shelf_kind.set(
                if st.shelf_kind.get_untracked() == k2 {
                    "all"
                } else {
                    &k2
                }
                .to_owned(),
            );
        },
        color,
    )
}

/// `TitleBox` — the card's title: double-click to rename; while editing an
/// input commits on Enter/blur (`renaming`/`shelf_rename_text` are store
/// signals so the state survives the card's rebuild).
fn title_box(i: ShelfItem) -> impl IntoElement {
    let name = i.name.clone();
    let title = i.title.clone();
    let title2 = i.title.clone();
    // The last-tap record `(name, ms)` — one shared slot; the name check
    // keeps two taps on different cards from counting as a rename.
    let last_tap = crate::live::keep("shelf-tap", || (String::new(), 0.0f64));
    dynamic(move || {
        let st = store();
        if st.shelf_renaming.get() == name {
            let name_s = name.clone();
            let name_b = name.clone();
            let title_b = title.clone();
            input()
                .auto_focus(true)
                .value(title.clone())
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ac("#f5f5f5"))
                .bg(ac("#f5f5f50f"))
                .rounded(px(5.0))
                .px(px(5.0))
                .py(px(1.0))
                .on_change(move |t: &str| store().shelf_rename_text.set(t.to_owned()))
                .on_submit(move |t: &str| commit_rename(&name_s, t))
                .on_blur(move || {
                    let t = store().shelf_rename_text.get_untracked();
                    let t = if t.is_empty() { title_b.clone() } else { t };
                    commit_rename(&name_b, &t);
                })
                .into_element()
        } else {
            let name_p = name.clone();
            let title_p = title2.clone();
            view()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .label("double-click to rename")
                .on_press(move || {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as f64)
                        .unwrap_or_default();
                    let (last_name, last) = last_tap.get_untracked();
                    if now - last < 380.0 && last_name == name_p {
                        store().shelf_renaming.set(name_p.clone());
                        store().shelf_rename_text.set(title_p.clone());
                    }
                    last_tap.set((name_p.clone(), now));
                })
                .child(
                    view().flex_1().child(
                        text(title2.clone())
                            .text_size(px(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ac("#f5f5f5"))
                            .number_of_lines(1),
                    ),
                )
                .child(text("✎").text_size(px(10.0)).text_color(ac("#f5f5f566")))
                .into_element()
        }
    })
}

/// `commit` — Enter/blur commit: trims, skips no-ops, POSTs the new title.
fn commit_rename(name: &str, t: &str) {
    let st = store();
    st.shelf_renaming.set(String::new());
    let t = t.trim();
    if t.is_empty() {
        return;
    }
    let item = st
        .shelf_list
        .get_untracked()
        .iter()
        .find(|i| i.name == name)
        .cloned();
    if item.map(|i| i.title).as_deref() != Some(t) {
        st.widget_meta(name, serde_json::json!({"title": t}));
    }
}

/// `TagBox` — the `+tag` pill that opens into a small input.
fn tag_box(i: ShelfItem) -> impl IntoElement {
    let name = i.name.clone();
    dynamic(move || {
        let st = store();
        if st.shelf_tagging.get() == name {
            let name_s = name.clone();
            let name_b = name.clone();
            input()
                .auto_focus(true)
                .placeholder("+tag")
                .text_size(px(10.0))
                .text_color(ac("#f5f5f5"))
                .w(px(56.0))
                .bg(ac("#f5f5f50f"))
                .rounded(px(6.0))
                .px(px(6.0))
                .py(px(2.0))
                .on_change(move |t: &str| store().shelf_tag_text.set(t.to_owned()))
                .on_submit(move |t: &str| commit_tag(&name_s, t))
                .on_blur(move || {
                    let t = store().shelf_tag_text.get_untracked();
                    commit_tag(&name_b, &t);
                })
                .into_element()
        } else {
            let name_p = name.clone();
            view()
                .rounded(px(999.0))
                .px(px(7.0))
                .py(px(2.0))
                .bg(ac("#f5f5f50f"))
                .on_press(move || {
                    store().shelf_tag_text.set(String::new());
                    store().shelf_tagging.set(name_p.clone());
                })
                .child(text("+tag").text_size(px(9.0)).text_color(ac("#f5f5f566")))
                .into_element()
        }
    })
}

/// `commit` — the tag joins the item's list (lowercased, trimmed).
fn commit_tag(name: &str, raw: &str) {
    let st = store();
    st.shelf_tagging.set(String::new());
    let t = raw.trim().to_lowercase();
    if t.is_empty() {
        return;
    }
    let Some(item) = st
        .shelf_list
        .get_untracked()
        .iter()
        .find(|i| i.name == name)
        .cloned()
    else {
        return;
    };
    let mut tags = item.tags.clone();
    if !tags.contains(&t) {
        tags.push(t);
    }
    st.widget_meta(name, serde_json::json!({"tags": tags}));
}

/// One shelf card (`card` in the TSX): tile, `name.tsx · date`, popout/menu
/// buttons, TitleBox, kind + tag chips, TagBox, the "＋ chat" reuse button.
fn shelf_card(i: ShelfItem) -> View {
    let name = i.name.clone();
    let kinds = i.kinds.clone();
    let tags = i.tags.clone();
    let first = i
        .title
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    let tile_color = i
        .kinds
        .first()
        .map(|k| kind_color(k))
        .unwrap_or_else(|| ac("#f5f5f585"));
    let sub = format!(
        "{}.tsx · {}",
        i.name,
        &i.published_at[..10.min(i.published_at.len())]
    );

    let i_ctx = i.clone();
    let i_menu = i.clone();
    let name_pop = name.clone();
    let name_reuse = name.clone();
    let name_reuse2 = name.clone();

    view()
        .w(percentage(0.48))
        .rounded(px(14.0))
        .border(px(1.0)).border_color(ac("#f5f5f51f"))
        .bg(ac("#1f1f1f"))
        .p(px(12.0))
        .gap(px(7.0))
        .on_context_menu(move |e: &ContextMenuEvent| {
            ctx::open(e, ctx::shelf_items(&i_ctx))
        })
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(
                    view()
                        .size(px(30.0))
                        .rounded(px(9.0))
                        .bg(tile_color)
                        .items_center()
                        .justify_center()
                        .child(
                            text(first)
                                .text_size(px(13.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(ac("#ffffff")),
                        ),
                )
                .child(
                    view()
                        .flex_1()
                        .child(
                            text(sub)
                                .text_size(px(9.5))
                                .text_color(ac("#f5f5f566"))
                                .number_of_lines(1),
                        ),
                )
                .child(act_btn("↗", move || store().popout(&name_pop)))
                .child(menu_btn(ctx::shelf_items(&i_menu))),
        )
        .child(title_box(i.clone()))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .flex_wrap()
                .children(kinds.iter().map(|k| {
                    let c = kind_color(k);
                    view()
                        .rounded(px(999.0))
                        .px(px(7.0))
                        .py(px(2.0))
                        .bg(c.opacity(0.15))
                        .child(
                            text(k.clone())
                                .text_size(px(9.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(c),
                        )
                }))
                .children(tags.iter().map(|t| {
                    let t2 = t.clone();
                    let name_t = name.clone();
                    view()
                        .rounded(px(999.0))
                        .px(px(7.0))
                        .py(px(2.0))
                        .bg(ac("#f5f5f50f"))
                        .on_press(move || {
                            // A tag chip's press removes it (index.tsx:384).
                            let mut tags = store()
                                .shelf_list
                                .get_untracked()
                                .iter()
                                .find(|x| x.name == name_t)
                                .map(|x| x.tags.clone())
                                .unwrap_or_default();
                            tags.retain(|x| *x != t2);
                            store().widget_meta(&name_t, serde_json::json!({"tags": tags}));
                        })
                        .child(
                            text(format!("#{t}"))
                                .text_size(px(9.0))
                                .text_color(ac("#f5f5f5a8")),
                        )
                }))
                .child(tag_box(i.clone()))
                .child(view().flex_1())
                .child(
                    view()
                        .rounded(px(8.0))
                        .px(px(9.0))
                        .py(px(4.0))
                        .bg(ac("#f5f5f514"))
                        .on_press(move || store().reuse_widget(&name_reuse))
                        .child(
                            text(move || {
                                if store().shelf_sent.get() == name_reuse2 {
                                    "＋ chat ✓".to_owned()
                                } else {
                                    "＋ chat".to_owned()
                                }
                            })
                            .text_size(px(10.5))
                            .text_color(ac("#f5f5f5")),
                        ),
                ),
        )
}

/// `WidgetShelf` — header (count + filter input), the kind/tag chip row,
/// the wrap grid of cards, and the empty state.
pub(crate) fn shelf() -> impl IntoElement {
    view()
        .gap(px(12.0))
        .px(px(16.0))
        .py(px(16.0))
        .w_full()
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    text(move || {
                        let n = store().shelf_shown().len();
                        format!("MINI APPS — {n}")
                    })
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ac("#f5f5f566")),
                )
                .child(
                    input()
                        .w(px(140.0))
                        .placeholder("filter…")
                        .text_size(px(11.0))
                        .text_color(ac("#f5f5f5"))
                        .bg(ac("#f5f5f50f"))
                        .rounded(px(8.0))
                        .px(px(10.0))
                        .py(px(4.0))
                        .on_change(move |t: &str| store().shelf_q.set(t.to_owned())),
                ),
        )
        .child(
            view()
                .flex_row()
                .gap(px(6.0))
                .flex_wrap()
                .child(chip(
                    "all".to_owned(),
                    || {
                        let st = store();
                        st.shelf_kind.get() == "all" && st.shelf_tag.get().is_empty()
                    },
                    || {
                        let st = store();
                        st.shelf_kind.set("all".to_owned());
                        st.shelf_tag.set(String::new());
                    },
                    "#f5f5f53d",
                ))
                .child(kind_chip("db"))
                .child(kind_chip("api"))
                .child(kind_chip("keys"))
                .child(kind_chip("mem"))
                .child(chip(
                    "archived".to_owned(),
                    || store().shelf_kind.get() == "arch",
                    || {
                        let st = store();
                        st.shelf_kind.set(
                            if st.shelf_kind.get_untracked() == "arch" {
                                "all"
                            } else {
                                "arch"
                            }
                            .to_owned(),
                        );
                    },
                    "#c95901",
                ))
                .child(dynamic(move || {
                    view()
                        .flex_row()
                        .gap(px(6.0))
                        .flex_wrap()
                        .children(store().shelf_tags().into_iter().map(|t| {
                            let t2 = t.clone();
                            let t3 = t.clone();
                            chip(
                                format!("#{t}"),
                                move || store().shelf_tag.get() == t2,
                                move || {
                                    let st = store();
                                    st.shelf_tag.set(
                                        if st.shelf_tag.get_untracked() == t3 {
                                            String::new()
                                        } else {
                                            t3.clone()
                                        },
                                    );
                                },
                                "#7c5e10",
                            )
                            .into_element()
                        }))
                        .into_element()
                })),
        )
        .child(dynamic(move || {
            let items = store().shelf_shown();
            let mut v = view().flex_row().gap(px(12.0)).flex_wrap().w_full();
            for i in items {
                v = v.child(shelf_card(i));
            }
            v.into_element()
        }))
        .child(
            when(
                || store().shelf_shown().is_empty(),
                || {
                    text("Nothing here — publish a generated card with ⬆, or loosen the filter.")
                        .text_size(px(13.0))
                        .text_color(ac("#f5f5f566"))
                },
            ),
        )
}

/// The work tab's body: a Scroll with the shelf centered at max-width 760
/// under the top bar (App.tsx:2144).
pub(crate) fn work_tab() -> impl IntoElement {
    scroll().flex_1().child(
        view()
            .self_center()
            .w_full()
            .max_w(px(760.0))
            .pt(px(52.0))
            .child(shelf()),
    )
}
