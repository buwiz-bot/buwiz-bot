//! The context-menu card (`ctxMenu` in the TSX shell): a styled popover
//! at the pointer — icon rows, hairline separators, red destructive
//! rows, checked rows and flyout submenus. A tap on the scrim or an
//! item closes it.
//!
//! Items carry a verb in `act`, a payload in `arg` and a toggle value
//! in `flag`; `act()` is the dispatch table the TSX built inline.

use nsh_core::ui::prelude::*;

use crate::icons::icon;
use crate::{live::store, store::CtxItem, store::CtxMenu, theme};

/// The TSX's metrics: CTX_ROW 30, CTX_SEP 9, CTX_PAD 5, CTX_W 220.
const CTX_W: f32 = 220.0;
const CTX_ROW: f32 = 30.0;
const CTX_SEP: f32 = 9.0;
const CTX_PAD: f32 = 5.0;

/// Run an item's verb — `onSelect` in the TSX menus.
fn act(it: &CtxItem) {
    let st = store();
    st.ctx.set(None);
    let (verb, arg) = (it.act.as_str(), it.arg.as_str());
    match verb {
        "new" => st.new_thread(),
        "palette" => st.palette.set(true),
        "models" => st.models_open.set(true),
        "chat-hide" => st.update_thread(arg, Some(it.flag), None),
        "chat-archive" => st.update_thread(arg, None, Some(it.flag)),
        "chat-del" => st.delete_thread(arg.to_owned()),
        "bot-pin" => st.update_bot(arg, serde_json::json!({"pinned": it.flag})),
        "bot-hide" => st.update_bot(arg, serde_json::json!({"hidden": it.flag})),
        "bot-del" => st.delete_bot(arg.to_owned()),
        "bot-move" => {
            let (id, sec) = arg.split_once('|').unwrap_or((arg, ""));
            st.move_to_section(id, sec);
        }
        "bot-section-prompt" => {
            st.section_name.set(String::new());
            st.section_prompt.set(Some(arg.to_owned()));
        }
        _ => {}
    }
}

/// `chatItems` — the chat row's menu (App.tsx:866): hide, archive,
/// delete. No Pin / Move to — bots own those.
pub(crate) fn chat_items(t: &crate::store::ThreadMeta) -> Vec<CtxItem> {
    vec![
        CtxItem::row(
            if t.hidden { "Show chat" } else { "Hide chat" },
            if t.hidden { "eye" } else { "eyeOff" },
            "chat-hide",
            t.id.clone(),
        )
        .to(!t.hidden),
        CtxItem::row(
            if t.archived { "Unarchive chat" } else { "Archive chat" },
            "archive",
            "chat-archive",
            t.id.clone(),
        )
        .to(!t.archived),
        CtxItem::separator(),
        CtxItem::danger("Delete", "trash", "chat-del", t.id.clone()),
    ]
}

/// `botItems` — a bot row's menu (App.tsx:831): pin, Move to ▸,
/// hide/show, delete.
pub(crate) fn bot_items(b: &crate::store::BotMeta) -> Vec<CtxItem> {
    let st = store();
    let mut sub: Vec<CtxItem> = st
        .sections
        .get_untracked()
        .iter()
        .map(|n| {
            CtxItem::row(n, "folder", "bot-move", format!("{}|{}", b.id, n))
                .on(b.section == *n)
        })
        .collect();
    sub.push(
        CtxItem::row("Unassigned", "folder", "bot-move", format!("{}|", b.id))
            .on(b.section.is_empty()),
    );
    sub.push(CtxItem::separator());
    sub.push(CtxItem::row(
        "Create section",
        "folder",
        "bot-section-prompt",
        b.id.clone(),
    ));
    vec![
        CtxItem::row(
            if b.pinned { "Unpin" } else { "Pin" },
            "pin",
            "bot-pin",
            b.id.clone(),
        )
        .to(!b.pinned),
        CtxItem::parent("Move to", "folder", sub),
        CtxItem::separator(),
        CtxItem::row(
            if b.hidden { "Show in sidebar" } else { "Hide from sidebar" },
            "eyeOff",
            "bot-hide",
            b.id.clone(),
        )
        .to(!b.hidden),
        CtxItem::separator(),
        CtxItem::danger("Delete", "trash", "bot-del", b.id.clone()),
    ]
}

/// `openCtx` — clamp the card inside the window like the TSX.
pub(crate) fn open(e: &ContextMenuEvent, items: Vec<CtxItem>) {
    let h = CTX_PAD * 2.0
        + items
            .iter()
            .map(|i| if i.sep { CTX_SEP } else { CTX_ROW })
            .sum::<f32>();
    // `winW()/windowSize` — conservative clamps; the scrim covers the rest.
    let x = (e.page_position.x.0 - 4.0).max(4.0);
    let y = (e.page_position.y.0 - 4.0).max(4.0).min(1400.0 - h);
    store().ctx.set(Some(CtxMenu { x, y, items }));
}

/// A row's top offset inside the card (`ctxRowTop`): rows before it
/// sum 30 or 9 for separators, plus the 5px pad.
fn row_top(items: &[CtxItem], idx: usize) -> f32 {
    CTX_PAD
        + items[..idx.min(items.len())]
            .iter()
            .map(|i| if i.sep { CTX_SEP } else { CTX_ROW })
            .sum::<f32>()
}

/// The overlay layer that shows the open `ctx` menu, if any.
pub(crate) fn layer() -> impl IntoElement {
    dynamic(|| {
        let Some(menu) = store().ctx.get() else {
            return view().into_element();
        };
        ctx_card(menu).into_element()
    })
}

fn ctx_card(menu: CtxMenu) -> impl IntoElement {
    let items = menu.items.clone();
    let items2 = menu.items.clone();
    // `ctxSub` — the main-row index whose flyout is open (or -1).
    let sub = signal(-1i64);
    let sub2 = sub;
    view()
        .size_full()
        .absolute()
        .child(
            // Scrim: dismiss on press or right-click anywhere.
            view()
                .size_full()
                .on_press(|| store().ctx.set(None))
                .on_context_menu(|_: &ContextMenuEvent| store().ctx.set(None)),
        )
        .child(
            view()
                .absolute()
                .left(px(menu.x))
                .top(px(menu.y))
                .w(px(CTX_W))
                .bg(theme::MENU_BG)
                .rounded(px(10.0))
                .border_1()
                .border_color(theme::BORDER_S)
                .py(px(CTX_PAD))
                .child(each(
                    move || items.clone(),
                    |it| it.label.clone(),
                    move |it, i| ctx_row(it.get(), i.get() as usize, sub),
                )),
        )
        .child(
            // The flyout: the hovered parent row's submenu, to the
            // card's right at the row's height (ctxSub in the TSX).
            dynamic(move || {
                let i = sub2.get();
                if i < 0 {
                    return view().into_element();
                }
                let it = &items2[i as usize];
                if it.submenu.is_empty() {
                    return view().into_element();
                }
                menu_card(
                    menu.x + CTX_W - 8.0,
                    menu.y + row_top(&items2, i as usize) - CTX_PAD,
                    it.submenu.clone(),
                )
                .into_element()
            }),
        )
}

/// One card of items — the main card and each flyout share it.
fn menu_card(x: f32, y: f32, items: Vec<CtxItem>) -> impl IntoElement {
    view()
        .absolute()
        .left(px(x))
        .top(px(y))
        .w(px(CTX_W))
        .bg(theme::MENU_BG)
        .rounded(px(10.0))
        .border_1()
        .border_color(theme::BORDER_S)
        .py(px(CTX_PAD))
        .child(each(
            move || items.clone(),
            |it| it.label.clone(),
            |it, _| ctx_row_leaf(it.get()),
        ))
}

/// The "Create section" prompt (`sectionPrompt`): a centered card with
/// a name input; OK moves the bot to the new section.
pub(crate) fn prompt_layer() -> impl IntoElement {
    dynamic(|| {
        let Some(bot_id) = store().section_prompt.get() else {
            return view().into_element();
        };
        let commit = move || {
            let st = store();
            let name = st.section_name.get_untracked().trim().to_owned();
            if !name.is_empty() {
                st.move_to_section(&bot_id, &name);
            }
            st.section_prompt.set(None);
        };
        let commit2 = commit.clone();
        view()
            .size_full()
            .absolute()
            .items_center()
            .justify_center()
            .child(
                view()
                    .size_full()
                    .bg(hsla(0.0, 0.0, 0.0, 0.4))
                    .on_press(|| store().section_prompt.set(None)),
            )
            .child(
                view()
                    .w(px(320.0))
                    .bg(theme::MENU_BG)
                    .rounded(px(12.0))
                    .border_1()
                    .border_color(theme::BORDER_S)
                    .p_4()
                    .gap_3()
                    .child(
                        text("New section")
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT),
                    )
                    .child(
                        input()
                            .value(move || store().section_name.get())
                            .placeholder("Section name")
                            .placeholder_color(|| color(theme::GHOST))
                            .text_size(px(13.0))
                            .text_color(theme::TEXT)
                            .on_change(move |v: &str| store().section_name.set(v.to_owned()))
                            .on_submit(move |_: &str| commit2())
                            .h(px(30.0)),
                    )
                    .child(
                        view()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(
                                text("Cancel")
                                    .text_size(px(13.0))
                                    .text_color(theme::MUTED)
                                    .px_3()
                                    .py_1()
                                    .rounded(px(7.0))
                                    .bg(theme::HOVER)
                                    .on_press(|| store().section_prompt.set(None)),
                            )
                            .child(
                                text("Create")
                                    .text_size(px(13.0))
                                    .text_color(white())
                                    .px_3()
                                    .py_1()
                                    .rounded(px(7.0))
                                    .bg(theme::ACCENT)
                                    .on_press(move || commit()),
                            ),
                    ),
            )
            .into_element()
    })
}

/// A flyout row (no nested submenu — the TSX's sub-items are leaves).
fn ctx_row_leaf(it: CtxItem) -> impl IntoElement {
    if it.sep {
        return view()
            .h(px(1.0))
            .mx_2()
            .my(px(4.0))
            .bg(theme::BORDER_S)
            .into_element();
    }
    let color = if it.danger { theme::DOWN } else { theme::TEXT };
    let hovered = signal(false);
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .h(px(CTX_ROW))
        .bg(move || {
            if hovered.get() {
                theme::HOVER
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .child(view().w(px(16.0)).items_center().child(icon(&it.icon)))
        .child(text(it.label.clone()).text_size(px(13.0)).text_color(color))
        .child(view().flex_1())
        .when(it.checked, |v| {
            v.child(text("✓").text_size(px(12.0)).text_color(theme::ACCENT))
        })
        .on_hover_in(move || hovered.set(true))
        .on_hover_out(move || hovered.set(false))
        .on_press(move || act(&it))
        .into_element()
}

/// A main-card row; `idx` feeds the card-level `sub` (`ctxSub`).
fn ctx_row(it: CtxItem, idx: usize, sub: Signal<i64>) -> impl IntoElement {
    if it.sep {
        return view()
            .h(px(1.0))
            .mx_2()
            .my(px(4.0))
            .bg(theme::BORDER_S)
            .into_element();
    }
    let idx = idx as i64;
    let color = if it.danger { theme::DOWN } else { theme::TEXT };
    let has_sub = !it.submenu.is_empty();
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .h(px(CTX_ROW))
        .bg(move || {
            if sub.get() == idx {
                theme::HOVER
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .child(view().w(px(16.0)).items_center().child(icon(&it.icon)))
        .child(text(it.label.clone()).text_size(px(13.0)).text_color(color))
        .child(view().flex_1())
        .when(it.checked, |v| {
            v.child(text("✓").text_size(px(12.0)).text_color(theme::ACCENT))
        })
        .when(has_sub, |v| {
            v.child(text("›").text_size(px(13.0)).text_color(theme::MUTED))
        })
        .on_hover_in(move || sub.set(idx))
        .on_hover_out(move || {
            if sub.get_untracked() == idx {
                sub.set(-1)
            }
        })
        .on_press(move || {
            if !has_sub {
                act(&it)
            }
        })
        .into_element()
}
