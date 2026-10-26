//! The ⌘K palette (`palette.tsx`): a centered card with a search field
//! listing the sessions first — the agents (with their avatars), then
//! the plain chats — then the mini apps pinned to the sidebar, then the
//! actions (`Store::palette_items`); arrows/enter drive `palette_sel`,
//! Escape or the scrim dismisses. As a picker (`palette_pick`) it lists
//! its own rows instead: the chats an item is attached in, the chats and
//! bots to attach one to.

use nsh_core::ui::prelude::*;

use crate::icons::{icon_ink, icon_sized};
use crate::plugins::hue;
use crate::live::store;
use crate::store::PaletteItem;
use crate::theme;

/// The overlay layer drawn while `palette` is open.
pub(crate) fn layer() -> impl IntoElement {
    dynamic(|| {
        if !store().palette.get() {
            return view().into_element();
        }
        palette_card().into_element()
    })
}

/// Grok Bot's palette: a 560pt card, at most 455pt tall, centered over a
/// 50% black backdrop.
const CARD_W: f32 = 560.0;
const CARD_H: f32 = 455.0;
const SEARCH_H: f32 = 51.0;
const ROW_H: f32 = 44.0;
const LIST_PAD: f32 = 6.0;

fn palette_card() -> impl IntoElement {
    let h = window().height.0;
    let list = element_ref();
    // Read once per change, not by every row and handler: the rows are a
    // virtualized list, so only those near the viewport are built.
    let results = computed(|| store().palette_results());
    // The list's scroll offset, to keep the arrow-key selection in view.
    let scroll_y = signal(0.0f32);
    let list_for_keys = list.clone();
    let card = view()
        .w(px(CARD_W))
        .max_h(px(CARD_H))
        .bg(theme::POPOVER)
        .rounded(px(14.0))
        .border_1()
        .border_color(theme::tc("#f5f5f51f"))
        .overflow_hidden()
        .child(
            // The search row (Grok Bot: 51pt, a 14pt magnifier at 52% ink).
            view()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .h(px(SEARCH_H))
                .pl(px(14.0))
                .pr(px(10.0))
                .border_b(px(0.5))
                .border_b_color(theme::BORDER_F)
                .child(icon_ink("search", 14.0, || theme::tc("#f5f5f585")))
                .child(
                    input()
                        .value(move || store().palette_q.get())
                        .on_change(|v: &str| {
                            let st = store();
                            st.palette_q.set(v.to_owned());
                            st.palette_sel.set(0);
                        })
                        .on_submit(|_: &str| {
                            let st = store();
                            st.palette_run_at(st.palette_sel.get_untracked() as usize);
                        })
                        // The field keeps the keys: Escape closes in one
                        // press, the arrows walk the results.
                        .on_key_press(move |key: &str| {
                            let st = store();
                            let step = match key {
                                "Escape" => return st.close_palette(),
                                "ArrowUp" => -1,
                                "ArrowDown" => 1,
                                _ => return,
                            };
                            let n = results.with_untracked(|r| r.len()) as i64;
                            if n == 0 {
                                return;
                            }
                            let sel = (st.palette_sel.get_untracked() + step).rem_euclid(n);
                            st.palette_sel.set(sel);
                            let view_h = CARD_H - SEARCH_H;
                            let top = LIST_PAD + sel as f32 * ROW_H;
                            let y = scroll_y.get_untracked();
                            let to = if top < y {
                                Some(top - LIST_PAD)
                            } else if top + ROW_H > y + view_h {
                                Some(top + ROW_H + LIST_PAD - view_h)
                            } else {
                                None
                            };
                            if let Some(to) = to {
                                list_for_keys.scroll_to_offset(px(to.max(0.0)), false);
                            }
                        })
                        .auto_focus(true)
                        .placeholder(|| {
                            let hint = store().palette_hint.get();
                            if hint.is_empty() { "Search".to_owned() } else { hint }
                        })
                        .placeholder_color(|| color(theme::GHOST))
                        .text_size(px(14.0))
                        .text_color(theme::TEXT)
                        .flex_1(),
                ),
        )
        .child(
            virtual_list(
                move || results.with(|r| r.len()),
                move |at| {
                    let blank = || PaletteItem {
                        title: String::new(),
                        subtitle: String::new(),
                        icon: String::new(),
                        checked: false,
                        act: crate::store::PaletteAct::NewChat,
                        bot: None,
                        letter_of: String::new(),
                        app_of: String::new(),
                    };
                    let row = computed(move || results.with(|r| r.get(at.index()).cloned()));
                    palette_row(move || row.get().unwrap_or_else(blank), move || at.index())
                },
            )
            .estimated_item_size(px(ROW_H))
            .content_padding(px(LIST_PAD))
            // A list has no intrinsic height: as tall as its rows, capped
            // by the card.
            .h(move || {
                let n = results.with(|r| r.len()) as f32;
                px((n * ROW_H + 2.0 * LIST_PAD).min(CARD_H - SEARCH_H))
            })
            .element_ref(&list)
            .on_scroll(move |e: &ScrollEvent| scroll_y.set(e.offset.y.0)),
        )
        .child(when(
            move || results.with(|r| r.is_empty()),
            || {
                text("No results")
                    .text_size(px(13.0))
                    .text_color(theme::GHOST)
                    .p(px(18.0))
                    .into_element()
            },
        ));
    view()
        .size_full()
        .absolute()
        .items_center()
        // The backdrop (Grok Bot: 50% black) — absolute so it doesn't
        // take column space above the card; a press dismisses.
        .child(
            view()
                .size_full()
                .absolute()
                .bg(hsla(0.0, 0.0, 0.0, 0.5))
                .on_press(|| store().close_palette()),
        )
        .child(card.mt(px(((h - CARD_H) / 2.0).max(40.0))))
}

fn palette_row(
    it: impl Fn() -> PaletteItem + Copy + 'static,
    ix: impl Fn() -> usize + Copy + 'static,
) -> impl IntoElement {
    let sel = move || store().palette_sel.get() as usize == ix();
    view()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .h(px(ROW_H))
                .px_2()
                .rounded(px(8.0))
                .bg(move || {
                    if sel() {
                        theme::HOVER
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .child(
                    // The 24pt tile: an agent's avatar, a chat's letter
                    // circle, a mini app's `</>` tile, or an action's icon.
                    view().size(px(24.0)).items_center().justify_center().child(dynamic(move || {
                        let item = it();
                        if let Some(b) = item.bot {
                            return crate::avatar::bot_avatar(crate::avatar::BotAvatarProps {
                                size: 24.0,
                                box_: Some(22.0),
                                paused: true,
                                ..crate::avatar::BotAvatarProps::bot(&b, 24.0)
                            })
                            .into_element();
                        }
                        if !item.letter_of.is_empty() {
                            let letter: String = item
                                .letter_of
                                .chars()
                                .next()
                                .map(|c| c.to_uppercase().collect())
                                .unwrap_or_default();
                            return view()
                                .size(px(22.0))
                                .rounded(px(11.0))
                                .items_center()
                                .justify_center()
                                .bg(hue(&item.letter_of))
                                .child(
                                    text(letter)
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(hsla(0.0, 0.0, 1.0, 1.0)),
                                )
                                .into_element();
                        }
                        if !item.app_of.is_empty() {
                            return crate::appdetails::app_icon(&item.app_of, 22.0).into_element();
                        }
                        let name: &'static str = match item.icon.as_str() {
                            "newChat" => "newChat",
                            "systemTheme" => "systemTheme",
                            "sun" => "sun",
                            "moon" => "moon",
                            _ => "gpts",
                        };
                        view()
                            .size(px(22.0))
                            .rounded(px(6.0))
                            .items_center()
                            .justify_center()
                            .bg(theme::SUBTLE)
                            .child(icon_sized(name, 13.0))
                            .into_element()
                    })),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(
                            text(move || it().title)
                                .text_size(px(13.0))
                                .line_height(px(18.0))
                                .text_color(theme::TEXT)
                                .number_of_lines(1),
                        )
                        .child(when(
                            move || !it().subtitle.is_empty(),
                            move || {
                                text(move || it().subtitle)
                                    .text_size(px(12.0))
                                    .line_height(px(16.0))
                                    .text_color(theme::MUTED)
                                    .number_of_lines(1)
                                    .into_element()
                            },
                        )),
                )
                .child(when(
                    move || it().checked,
                    || icon_sized("check", 13.0).into_element(),
                ))
                .child(when(
                    move || {
                        !store().palette_q.get().trim().is_empty() && ix() < 9
                    },
                    move || {
                        text(move || format!("⌘{}", ix() + 1))
                            .text_size(px(11.0))
                            .text_color(theme::GHOST)
                            .into_element()
                    },
                )),
        )
        .on_hover_in(move || store().palette_sel.set(ix() as i64))
        .on_press(move || store().palette_run_at(ix()))
        .into_element()
}

