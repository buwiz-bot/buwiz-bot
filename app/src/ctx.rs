//! The context-menu card (`ctxMenu` in the TSX shell): a styled popover
//! at the pointer — icon rows, hairline separators, red destructive
//! rows, checked rows and flyout submenus. A tap on the scrim or an
//! item closes it.
//!
//! Items carry a verb in `act`, a payload in `arg` and a toggle value
//! in `flag`; `act()` is the dispatch table the TSX built inline.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

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
        "bot-modal" => st.bot_modal.set(true),
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
        "app-rename-title" | "app-rename-file" | "app-rename-label" => {
            st.app_action(verb, arg.to_owned())
        }
        "publish" | "unpublish" | "app-delete" | "app-delete-everywhere" => {
            st.app_action(verb, arg.to_owned())
        }
        "mcp-enable" | "mcp-disable" | "mcp-reconnect" | "mcp-delete" => {
            st.mcp_verb(verb, arg.to_owned())
        }
        // file turns (`⋯` on an artifact): copy arbitrary text or open a
        // reply file by `turn_i|path`.
        "copy" => st.copy_text(arg),
        "file-open" => {
            let (i_s, path) = arg.split_once('|').unwrap_or(("", arg));
            let i: i64 = i_s.parse().unwrap_or(-1);
            if let Some(t) = st.turns.get_untracked().iter().find(|t| t.i == i) {
                if let Some(f) = t
                    .files
                    .iter()
                    .chain(t.attachments.iter())
                    .find(|f| f.path == path)
                {
                    let f = f.clone();
                    st.open_file(&f);
                }
            }
        }
        "shelf-popout" => st.popout(arg),
        "shelf-pin-desktop" => st.popout_desktop(arg),
        "shelf-rename" => {
            st.shelf_rename_text.set(String::new());
            st.shelf_renaming.set(arg.to_owned());
        }
        "shelf-tag" => {
            st.shelf_tag_text.set(String::new());
            st.shelf_tagging.set(arg.to_owned());
        }
        "shelf-regen" => st.retry_widget(arg),
        "shelf-pin" => st.pin_widget(arg, it.flag),
        "shelf-archive" => st.widget_meta(arg, serde_json::json!({"archived": it.flag})),
        "shelf-delete" => st.unpublish(arg),
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

/// The `⋯` menu under a reply artifact (shell.tsx:360): Open + Copy file
/// name only when the artifact is a single file; Copy message always.
/// `i` is the turn index the file lookup scopes to (`file-open`).
pub(crate) fn artifact_items(
    i: i64,
    f: Option<&crate::store::Attach>,
    message: &str,
) -> Vec<CtxItem> {
    let mut items: Vec<CtxItem> = Vec::new();
    if let Some(f) = f {
        items.push(CtxItem::row(
            "Open",
            "eye",
            "file-open",
            format!("{i}|{}", f.path),
        ));
        items.push(CtxItem::row(
            "Copy file name",
            "copy",
            "copy",
            f.name.clone(),
        ));
    }
    items.push(CtxItem::row(
        "Copy message",
        "copy",
        "copy",
        message.to_owned(),
    ));
    items
}

/// `botItems` — a bot row's menu (App.tsx:831): pin, Move to ▸,
/// hide/show, delete.
pub(crate) fn bot_items(b: &crate::store::RoxBot) -> Vec<CtxItem> {
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

/// `addMenu` (App.tsx:979) — the "+" button's two-item menu.
pub(crate) fn add_items() -> Vec<CtxItem> {
    vec![
        CtxItem::row("New bot", "newChat", "bot-modal", String::new()),
        CtxItem::row("New chat", "edit", "new", String::new()),
    ]
}

/// `appMenu` — a library row's menu (App.tsx:1639): rename title/file,
/// publish, delete.
pub(crate) fn app_items(a: &crate::store::RoxApp) -> Vec<CtxItem> {
    let published = a.published;
    let mut items = vec![
        CtxItem::row("Rename Title…", "edit", "app-rename-title", a.app.clone()),
        CtxItem::row("Rename File Name…", "edit", "app-rename-file", a.app.clone()),
        CtxItem::separator(),
        CtxItem::row(
            if published { "Remove from Mini Apps" } else { "Promote to Mini Apps" },
            "gpts",
            if published { "unpublish" } else { "publish" },
            a.app.clone(),
        ),
        CtxItem::separator(),
        CtxItem::danger("Delete…", "trash", "app-delete", a.app.clone()),
    ];
    if published {
        items.push(CtxItem::danger(
            "Delete Everywhere…",
            "trash",
            "app-delete-everywhere",
            a.app.clone(),
        ));
    }
    items
}

/// The shelf card's menu (`openMenu` in widgets/index.tsx:227): popout,
/// desktop pin, rename/tag editors, regenerate, nominate, archive, delete.
pub(crate) fn shelf_items(i: &crate::store::ShelfItem) -> Vec<CtxItem> {
    vec![
        CtxItem::row("Open window", "popout", "shelf-popout", i.name.clone()),
        CtxItem::row("Pin to desktop", "pin", "shelf-pin-desktop", i.name.clone()),
        CtxItem::separator(),
        CtxItem::row("Rename…", "edit", "shelf-rename", i.name.clone()),
        CtxItem::row("Add tag…", "tag", "shelf-tag", i.name.clone()),
        CtxItem::row("Regenerate", "refresh", "shelf-regen", i.name.clone()),
        CtxItem::row(
            if i.pinned { "Unpin" } else { "Nominate (★)" },
            "pin",
            "shelf-pin",
            i.name.clone(),
        )
        .to(!i.pinned),
        CtxItem::row(
            if i.archived { "Unarchive" } else { "Archive" },
            "archive",
            "shelf-archive",
            i.name.clone(),
        )
        .to(!i.archived),
        CtxItem::separator(),
        CtxItem::danger(
            "Delete from Mini Apps",
            "trash",
            "shelf-delete",
            i.name.clone(),
        ),
    ]
}

/// `mcpMenu` — a connected server's menu (App.tsx:1602): enable,
/// reconnect, remove.
pub(crate) fn mcp_items(s: &crate::store::McpServer) -> Vec<CtxItem> {
    vec![
        CtxItem::row(
            if s.enabled { "Disable" } else { "Enable" },
            if s.enabled { "powerOff" } else { "power" },
            if s.enabled { "mcp-disable" } else { "mcp-enable" },
            s.id.clone(),
        ),
        CtxItem::row("Reconnect", "refresh", "mcp-reconnect", s.id.clone()),
        CtxItem::separator(),
        CtxItem::danger("Remove…", "trash", "mcp-delete", s.id.clone()),
    ]
}

/// `openCtx` — clamp the card inside the window like the TSX.
pub(crate) fn open(e: &ContextMenuEvent, items: Vec<CtxItem>) {
    open_at(e.page_position.x.0, e.page_position.y.0, items)
}

/// The same open at explicit page coords — the add-menu's measured
/// button position (App.tsx's `addMenu`).
pub(crate) fn open_at(x: f32, y: f32, items: Vec<CtxItem>) {
    let h = CTX_PAD * 2.0
        + items
            .iter()
            .map(|i| if i.sep { CTX_SEP } else { CTX_ROW })
            .sum::<f32>();
    // `winW()/windowSize` — conservative clamps; the scrim covers the rest.
    let x = (x - 4.0).max(4.0);
    let y = (y - 4.0).max(4.0).min(1400.0 - h);
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
    // `ctxSub` — the main-row index whose flyout is open (or -1). It
    // only moves on row hover-in, never on hover-out, so the pointer can
    // leave the parent row and reach the flyout.
    let sub = signal(-1i64);
    let sub2 = sub;
    // `ctxSubT` — while a flyout is open, retargeting `sub` is delayed
    // 350ms so crossing sibling rows on the way in doesn't close it; a
    // flyout row's hover-in cancels the pending move. `pending` is the
    // generation the delayed task must still see when it wakes.
    let pending = Rc::new(Cell::new(0u64));
    let sub_set = {
        let pending = pending.clone();
        Rc::new(move |idx: i64| {
            let cur = sub.get_untracked();
            let tok = pending.get().wrapping_add(1);
            pending.set(tok);
            if cur >= 0 && idx != cur {
                let pending = pending.clone();
                spawn(async move {
                    timer(Duration::from_millis(350)).await;
                    if pending.get() == tok {
                        sub.set(idx);
                    }
                });
            } else {
                sub.set(idx);
            }
        }) as Rc<dyn Fn(i64)>
    };
    let sub_cancel = {
        let pending = pending.clone();
        Rc::new(move || pending.set(pending.get().wrapping_add(1))) as Rc<dyn Fn()>
    };
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
                    move |it, i| {
                        ctx_row(it.get(), i.get() as usize, sub, sub_set.clone())
                    },
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
                    sub_cancel.clone(),
                )
                .into_element()
            }),
        )
}

/// One card of items — the main card and each flyout share it.
/// `cancel` is the flyout rows' `isSub` hover-in: entering a flyout row
/// cancels any pending `sub` move so the flyout stays open.
fn menu_card(x: f32, y: f32, items: Vec<CtxItem>, cancel: Rc<dyn Fn()>) -> impl IntoElement {
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
            move |it, _| ctx_row_leaf(it.get(), cancel.clone()),
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
/// `isSub` rows never touch `ctxSub`; their hover-in cancels a pending
/// close so the pointer can arrive from the parent row.
fn ctx_row_leaf(it: CtxItem, cancel: Rc<dyn Fn()>) -> impl IntoElement {
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
        .on_hover_in(move || {
            cancel();
            hovered.set(true)
        })
        .on_hover_out(move || hovered.set(false))
        .on_press(move || act(&it))
        .into_element()
}

/// A main-card row. Hover-in is `ctxSubSet(it.submenu ? idx : -1)`;
/// `sub` never moves on hover-out. The highlight is the row's own
/// hover or an open flyout pointing back at it.
fn ctx_row(
    it: CtxItem,
    idx: usize,
    sub: Signal<i64>,
    sub_set: Rc<dyn Fn(i64)>,
) -> impl IntoElement {
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
    let hovered = signal(false);
    let sub_set2 = sub_set.clone();
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .h(px(CTX_ROW))
        .bg(move || {
            if hovered.get() || has_sub && sub.get() == idx {
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
        .on_hover_in(move || {
            hovered.set(true);
            sub_set(if has_sub { idx } else { -1 })
        })
        .on_hover_out(move || hovered.set(false))
        .on_press(move || {
            if has_sub {
                sub_set2(idx)
            } else {
                act(&it)
            }
        })
        .into_element()
}
