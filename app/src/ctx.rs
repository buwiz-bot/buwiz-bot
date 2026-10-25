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
pub(crate) const CTX_W: f32 = 220.0;
const CTX_ROW: f32 = 30.0;
const CTX_SEP: f32 = 9.0;
const CTX_PAD: f32 = 5.0;

/// Run an item's verb — `onSelect` in the TSX menus. Detached from the
/// menu: closing it disposes the row's owner, which cancelled any task a
/// verb spawned there (Delete never reached the server).
fn act(it: &CtxItem) {
    let it = it.clone();
    store().ctx.set(None);
    Owner::detached().run(move || run(&it));
}

fn run(it: &CtxItem) {
    let st = store();
    let (verb, arg) = (it.act.as_str(), it.arg.as_str());
    match verb {
        "new" => st.new_thread(),
        "bot-modal" => st.bot_modal.set(true),
        "palette" => st.palette.set(true),
        "models" => crate::models::open_list(),
        "chat-hide" => st.update_thread(arg, Some(it.flag), None),
        "chat-archive" => st.update_thread(arg, None, Some(it.flag)),
        "chat-del" => st.delete_thread(arg.to_owned()),
        "bot-pin" => st.update_bot(arg, serde_json::json!({"pinned": it.flag})),
        "main-set" => st.set_main_bot(arg),
        "bot-int-add" => {
            let (bot, integration) = arg.split_once('|').unwrap_or(("", arg));
            st.bot_integration(bot, integration, true);
        }
        "main-replace" => {
            st.main_pick_q.set(String::new());
            st.main_pick.set(Some(arg.to_owned()));
        }
        "bot-hide" => st.update_bot(arg, serde_json::json!({"hidden": it.flag})),
        "bot-del" => st.delete_bot(arg.to_owned()),
        // Groups' verbs carry their fields joined by U+001F (a file path
        // may hold anything else): `<group>␟<kind>␟<id>␟<origin chat>`.
        "group-item" => {
            let f: Vec<&str> = arg.split('\u{1f}').collect();
            if let [gid, kind, id, thread] = f[..] {
                st.group_item(gid, kind, id, thread, it.flag);
            }
        }
        // Move to Group ›: `<from group>␟<to group>␟<kind>␟<id>␟<origin chat>`.
        "group-move" => {
            let f: Vec<&str> = arg.split('\u{1f}').collect();
            if let [from, to, kind, id, thread] = f[..] {
                st.group_place(to, kind, id, thread, None, Some(from));
            }
        }
        // `<kind>␟<id>␟<origin chat>`, or "" for an empty group.
        "group-new" => {
            let f: Vec<&str> = arg.split('\u{1f}').collect();
            let item = match f[..] {
                [kind, id, thread] => Some((kind.to_owned(), id.to_owned(), thread.to_owned())),
                _ => None,
            };
            st.group_name.set(String::new());
            st.group_prompt.set(Some(crate::store::GroupPrompt::New(item)));
        }
        "group-rename" => {
            let name = st.groups.with_untracked(|gs| gs.iter().find(|g| g.id == arg).map(|g| g.name.clone()));
            st.group_name.set(name.unwrap_or_default());
            st.group_prompt.set(Some(crate::store::GroupPrompt::Rename(arg.to_owned())));
        }
        // The fixed sections' header menu: "__un" or "__chats".
        "section-rename" => {
            let name = match arg {
                "__un" => st.unassigned_name.get_untracked(),
                _ => st.chats_name.get_untracked(),
            };
            st.group_name.set(name);
            st.group_prompt.set(Some(crate::store::GroupPrompt::RenameSection(arg.to_owned())));
        }
        "group-color-custom" => {
            let cur = st.groups.with_untracked(|gs| gs.iter().find(|g| g.id == arg).map(|g| g.color.clone()));
            st.color_draft.set(cur.unwrap_or_default());
            st.color_edit.set(Some(arg.to_owned()));
        }
        "group-color" => {
            if let Some((gid, color)) = arg.split_once('\u{1f}') {
                st.update_group(gid, serde_json::json!({ "color": color }));
            }
        }
        "group-fold" => st.toggle_section(arg),
        "group-del" => st.delete_group(arg),
        // A group's artifact: `<group>␟<id>`.
        "group-open" | "group-show" | "group-details" | "group-download" => {
            let Some((gid, id)) = arg.split_once('\u{1f}') else { return };
            let Some(a) = group_artifact(gid, id) else { return };
            match verb {
                "group-open" => st.open_library_item(&a),
                "group-show" => st.show_group_item(&a),
                "group-details" => st.open_details_of(&a.app, &a.kind),
                _ => st.save_library_item(&a),
            }
        }
        "app-details" => st.open_app_details(arg),
        "app-rename-label" => {
            st.app_action(verb, arg.to_owned())
        }
        "publish" | "unpublish" | "app-delete" | "app-delete-everywhere" => {
            st.app_action(verb, arg.to_owned())
        }
        // file turns (`⋯` on an artifact): copy arbitrary text or open a
        // reply file by `turn_i|path`.
        "copy" => st.copy_text(arg),
        "card-details" => st.show_in_library(arg),
        // A card's verbs carry `<turn i>|<module key>`.
        "card-popout" => {
            let (i_s, app) = arg.split_once('|').unwrap_or(("", arg));
            let i: i64 = i_s.parse().unwrap_or(-1);
            let (w, h) = st.card_size(i, app);
            st.popout_sized(app, w, h);
        }
        "card-retry" => st.retry_card(arg),
        // `more()`'s Delete (shell.tsx:307): `post("/turn/delete", {key})`.
        "turn-del" => {
            crate::client::post_bg(
                "/turn/delete".to_owned(),
                serde_json::json!({"key": arg}),
            );
            st.refresh_turns();
        }
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
        "plugin-cat" => st.plugin_cat.set(arg.to_owned()),
        // A Setup page select: "KEY\u{1f}value".
        "setup-opt" => {
            if let Some((k, v)) = arg.split_once('\u{1f}') {
                let (k, v) = (k.to_owned(), v.to_owned());
                st.setup_vals.update(|m| {
                    m.insert(k, v);
                });
            }
        }
        "lib-open" => {
            if let Some(a) = st.thread_apps.with_untracked(|v| v.iter().find(|a| a.app == arg).cloned()) {
                st.open_library_item(&a);
            }
        }
        "lib-show" => st.show_library_item(arg),
        "lib-details" => st.open_details(arg),
        // `<module key>␟<file name>`: a mini app's source in the viewer
        "app-code" => {
            let (key, file) = arg.split_once('\u{1f}').unwrap_or((arg, ""));
            st.view_code(key, file);
        }
        "viewer-print" => crate::files::viewer_print(),
        "viewer-save" => crate::files::viewer_save(),
        "viewer-version" => crate::files::viewer_open_version(arg.parse().unwrap_or(0)),
        "viewer-download" => {
            if let Some(f) = st.viewing.get_untracked() {
                st.save_file(&f.path);
            }
        }
        "viewer-open" => {
            if let Some(f) = st.viewing.get_untracked() {
                st.open_externally(&f.path);
            }
        }
        "viewer-close" => st.viewing.set(None),
        "lib-hide" => st.archive_library_item(arg, true),
        "lib-unhide" => st.archive_library_item(arg, false),
        "lib-delete" => st.delete_library_file(arg),
        "lib-download" => {
            if let Some(a) = st.thread_apps.with_untracked(|v| v.iter().find(|a| a.app == arg).cloned()) {
                st.save_library_item(&a);
            }
        }
        // `<x>|<y>|<id>` — the picker opens where the menu was.
        "lib-versions" => {
            let mut it = arg.splitn(3, '|');
            let x = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
            let y = it.next().and_then(|y| y.parse().ok()).unwrap_or(0.0);
            st.library_versions(it.next().unwrap_or_default(), x, y);
        }
        _ => {}
    }
}

/// The color dots a group picks from (the server's `GROUP_COLORS`).
pub(crate) const GROUP_COLORS: [(&str, &str); 8] = [
    ("Blue", "#1884fc"),
    ("Green", "#32bb78"),
    ("Orange", "#f2994a"),
    ("Red", "#eb5757"),
    ("Purple", "#9b51e0"),
    ("Teal", "#2d9cdb"),
    ("Yellow", "#f2c94c"),
    ("Gray", "#8e8e93"),
];

/// "Add to Group ›" for item `kind`/`id` (an artifact with its origin
/// chat `thread`): every group, checked where the item already is (a
/// press adds or takes it out of that one), then New Group….
pub(crate) fn group_menu(kind: &str, id: &str, thread: &str) -> CtxItem {
    let st = store();
    let mut seen: Vec<String> = Vec::new();
    let mut sub: Vec<CtxItem> = st
        .groups
        .get_untracked()
        .iter()
        .map(|g| {
            // flyout rows are keyed by label: two groups of one name differ
            let mut label = g.name.clone();
            let n = seen.iter().filter(|x| **x == g.name).count();
            seen.push(g.name.clone());
            if n > 0 {
                label = format!("{label} ({})", n + 1);
            }
            let on = st.in_group(&g.id, kind, id);
            CtxItem::row(&label, &g.color, "group-item", [g.id.as_str(), kind, id, thread].join("\u{1f}"))
                .on(on)
                .to(!on)
        })
        .collect();
    if !sub.is_empty() {
        sub.push(CtxItem::separator());
    }
    sub.push(CtxItem::row("New Group…", "folderPlus", "group-new", [kind, id, thread].join("\u{1f}")));
    CtxItem::parent("Add to Group", "folderPlus", sub)
}

/// "Move to Group ›" for item `kind`/`id` of group `from`: the other
/// groups; a press puts it at the end of that one and takes it out of
/// `from` (a drag only copies it). None with no other group.
pub(crate) fn move_menu(from: &str, kind: &str, id: &str, thread: &str) -> Option<CtxItem> {
    let mut seen: Vec<String> = Vec::new();
    let sub: Vec<CtxItem> = store()
        .groups
        .get_untracked()
        .iter()
        .filter(|g| g.id != from)
        .map(|g| {
            let mut label = g.name.clone();
            let n = seen.iter().filter(|x| **x == g.name).count();
            seen.push(g.name.clone());
            if n > 0 {
                label = format!("{label} ({})", n + 1);
            }
            CtxItem::row(&label, &g.color, "group-move", [from, g.id.as_str(), kind, id, thread].join("\u{1f}"))
        })
        .collect();
    (!sub.is_empty()).then(|| CtxItem::parent("Move to Group", "folder", sub))
}

/// A row's menu as it shows inside group `from`: Move to Group › right
/// under Add to Group ›.
pub(crate) fn with_move(mut items: Vec<CtxItem>, from: &str, kind: &str, id: &str, thread: &str) -> Vec<CtxItem> {
    if let Some(m) = move_menu(from, kind, id, thread) {
        let at = items.iter().position(|it| it.label == "Add to Group").map_or(items.len(), |i| i + 1);
        items.insert(at, m);
    }
    items
}

/// Group `gid`'s artifact `id`, as the sidebar holds it.
fn group_artifact(gid: &str, id: &str) -> Option<crate::store::RoxApp> {
    store().groups.with_untracked(|gs| {
        gs.iter()
            .find(|g| g.id == gid)?
            .items
            .iter()
            .find(|it| it.id == id)?
            .artifact
            .clone()
    })
}

/// A group header's menu: rename, its color, fold, delete (the items
/// stay).
pub(crate) fn group_header_items(g: &crate::store::RoxGroup) -> Vec<CtxItem> {
    let folded = untracked(|| store().collapsed.get().get(&g.id).copied().unwrap_or(false));
    let preset = GROUP_COLORS.iter().any(|(_, hex)| g.color.eq_ignore_ascii_case(hex));
    let mut colors: Vec<CtxItem> = GROUP_COLORS
        .iter()
        .map(|(name, hex)| {
            CtxItem::row(name, hex, "group-color", format!("{}\u{1f}{hex}", g.id))
                .on(g.color.eq_ignore_ascii_case(hex))
        })
        .collect();
    // any other color: the picker (checked when the group has one)
    colors.push(CtxItem::separator());
    colors.push(CtxItem::row("Custom…", if preset { "plus" } else { &g.color }, "group-color-custom", g.id.clone()).on(!preset));
    vec![
        CtxItem::row("Rename…", "edit", "group-rename", g.id.clone()),
        CtxItem::parent("Color", &g.color, colors),
        CtxItem::row(if folded { "Expand" } else { "Collapse" }, "folderOpen", "group-fold", g.id.clone()),
        CtxItem::separator(),
        CtxItem::danger("Delete Group", "trash", "group-del", g.id.clone()),
    ]
}

/// A fixed section's header menu (Unassigned, Chats): rename, fold.
pub(crate) fn section_header_items(key: &str) -> Vec<CtxItem> {
    let folded = untracked(|| store().collapsed.get().get(key).copied().unwrap_or(false));
    vec![
        CtxItem::row("Rename…", "edit", "section-rename", key.to_owned()),
        CtxItem::row(if folded { "Expand" } else { "Collapse" }, "folderOpen", "group-fold", key.to_owned()),
    ]
}

/// A group's artifact row: open it (from any chat), its chat, its
/// details, download (images and files), its link, the groups — and out
/// of this one.
pub(crate) fn group_art_items(gid: &str, a: &crate::store::RoxApp) -> Vec<CtxItem> {
    let at = format!("{gid}\u{1f}{}", a.app);
    let thread = store().groups.with_untracked(|gs| {
        gs.iter()
            .find(|g| g.id == gid)
            .and_then(|g| g.items.iter().find(|it| it.kind == "artifact" && it.id == a.app))
            .map(|it| it.thread.clone())
            .unwrap_or_default()
    });
    let remote = a.storage.connector();
    let mut items = vec![
        CtxItem::row(if remote { "Open in Drive" } else { "Open" }, "popout", "group-open", at.clone()),
        CtxItem::row("Show in Chat", "quoteBubble", "group-show", at.clone()),
        CtxItem::row(
            if a.kind == "app" { "Edit App Details…" } else { "Edit Details…" },
            "edit",
            "group-details",
            at.clone(),
        ),
    ];
    if a.kind == "app" {
        items.push(CtxItem::row("View Code", "code", "app-code", format!("{}\u{1f}{}", a.app, a.filename)));
    }
    // only a file on this Mac downloads; Drive keeps its own
    if a.kind != "app" && a.storage.place == "mac" {
        items.push(CtxItem::row("Download", "download", "group-download", at.clone()));
    }
    items.push(CtxItem::row("Copy Link", "chainLink", "copy", store().library_link(a)));
    items.push(CtxItem::separator());
    items.push(group_menu("artifact", &a.app, &thread));
    if let Some(m) = move_menu(gid, "artifact", &a.app, &thread) {
        items.push(m);
    }
    items.push(
        CtxItem::row(
            "Remove from Group",
            "folder",
            "group-item",
            [gid, "artifact", a.app.as_str(), ""].join("\u{1f}"),
        )
        .to(false),
    );
    items
}

/// `chatItems` — the chat row's menu (App.tsx:866): its groups, hide,
/// archive, delete.
pub(crate) fn chat_items(t: &crate::store::ThreadMeta) -> Vec<CtxItem> {
    vec![
        group_menu("chat", &t.id, ""),
        CtxItem::separator(),
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
        let thread = store().active_id.get_untracked();
        let id = format!("file:{}", f.path);
        items.push(group_menu("artifact", &id, &thread));
        items.push(archive_item(&id));
    }
    items.push(CtxItem::row(
        "Copy message",
        "copy",
        "copy",
        message.to_owned(),
    ));
    items
}

/// `CardShell`'s ⋯ menu (shell.tsx:806) — the card's actions live here,
/// not in a row inside the card: details, its groups, open in a window,
/// try again, delete.
pub(crate) fn card_items(i: i64, app: &str) -> Vec<CtxItem> {
    let at = format!("{i}|{app}");
    let groups = group_menu("artifact", crate::store::app_name(app), &store().active_id.get_untracked());
    // An image card is no mini app: no app details or window.
    let image = store().turns.with_untracked(|t| {
        t.iter().find(|t| t.i == i).is_some_and(|t| t.art.as_ref().is_some_and(|a| a.kind == "image"))
    });
    let archive = archive_item(crate::store::app_name(app));
    if image {
        return vec![
            groups,
            archive,
            CtxItem::separator(),
            CtxItem::row("Try Again", "retry", "card-retry", app.to_owned()),
            CtxItem::row("Copy Link", "chainLink", "copy", store().turn_link(i)),
            CtxItem::separator(),
            CtxItem::danger("Delete", "trash", "turn-del", format!("idx:{i}")),
        ];
    }
    vec![
        CtxItem::row("Details", "panel", "card-details", app.to_owned()),
        CtxItem::row("Edit App Details…", "edit", "app-details", app.to_owned()),
        groups,
        archive,
        CtxItem::separator(),
        CtxItem::row("Open in Window", "popout", "card-popout", at),
        // the version this card shows
        CtxItem::row("View Code", "code", "app-code", format!("{app}\u{1f}")),
        CtxItem::separator(),
        CtxItem::row("Try Again", "retry", "card-retry", app.to_owned()),
        CtxItem::row("Copy Link", "chainLink", "copy", store().turn_link(i)),
        CtxItem::separator(),
        CtxItem::danger("Delete", "trash", "turn-del", format!("idx:{i}")),
    ]
}

/// A chat card's Library state: Archive it, or — archived — Show in
/// Library again.
fn archive_item(id: &str) -> CtxItem {
    if store().is_archived(id) {
        CtxItem::row("Show in Library", "eye", "lib-unhide", id.to_owned())
    } else {
        CtxItem::row("Archive", "eyeOff", "lib-hide", id.to_owned())
    }
}

/// `more()` on a message row (shell.tsx:301): Copy + Delete; `i` is the
/// turn index, posted as `key: "idx:<i>"`.
pub(crate) fn msg_items(i: i64) -> Vec<CtxItem> {
    let text = store()
        .turns
        .get_untracked()
        .iter()
        .find(|t| t.i == i)
        .map(|t| t.text.clone())
        .unwrap_or_default();
    vec![
        CtxItem::row("Copy", "copy", "copy", text),
        CtxItem::row("Copy Link", "chainLink", "copy", store().turn_link(i)),
        CtxItem::separator(),
        CtxItem::danger("Delete", "trash", "turn-del", format!("idx:{i}")),
    ]
}

/// `botItems` — a bot row's menu (App.tsx:831): pin to the top tiles,
/// its groups, hide/show, delete.
pub(crate) fn bot_items(b: &crate::store::RoxBot) -> Vec<CtxItem> {
    let st = store();
    vec![
        CtxItem::row(
            if b.pinned { "Unpin from Top" } else { "Pin to Top" },
            "pin",
            "bot-pin",
            b.id.clone(),
        )
        .to(!b.pinned),
        group_menu("bot", &b.id, ""),
        CtxItem::separator(),
        CtxItem::row(
            if b.hidden { "Show in sidebar" } else { "Hide from sidebar" },
            "eyeOff",
            "bot-hide",
            b.id.clone(),
        )
        .to(!b.hidden),
        CtxItem::separator(),
        // The primary Bot (Grok Bot's star): set it here, or from the
        // current one, pick another.
        if untracked(|| st.is_main_bot(&b.id)) {
            CtxItem::row("Replace with different Bot", "refresh", "main-replace", b.id.clone())
        } else {
            CtxItem::row("Set as Main Bot", "pin", "main-set", b.id.clone())
        },
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

/// `appMenu` — a library row's menu (App.tsx:1639), by kind: open, back
/// to the chat, details, versions, download, link; then its groups,
/// Remove from Library and an app's deletion. `at` is the menu's
/// page position — the Versions picker opens there.
pub(crate) fn app_items(a: &crate::store::RoxApp, at: (f32, f32)) -> Vec<CtxItem> {
    let app = a.kind == "app";
    let id = a.app.clone();
    // A connector's file (Google Drive) opens there, copies its Drive link
    // and has neither Rox versions nor a download.
    let remote = a.storage.connector();
    let mut items = vec![
        CtxItem::row(if remote { "Open in Drive" } else { "Open" }, "popout", "lib-open", id.clone()),
        CtxItem::row("Show in Chat", "quoteBubble", "lib-show", id.clone()),
        CtxItem::row(
            if app { "Edit App Details…" } else { "Edit Details…" },
            "edit",
            "lib-details",
            id.clone(),
        ),
    ];
    if app {
        items.push(CtxItem::row("View Code", "code", "app-code", format!("{id}\u{1f}{}", a.filename)));
    }
    if a.v > 0 && !a.versions.is_empty() {
        items.push(CtxItem::row(
            "Versions",
            "history",
            "lib-versions",
            format!("{}|{}|{id}", at.0, at.1),
        ));
    }
    if !app && a.storage.place == "mac" {
        items.push(CtxItem::row("Download", "download", "lib-download", id.clone()));
    }
    items.push(CtxItem::row("Copy Link", "chainLink", "copy", store().library_link(a)));
    items.push(CtxItem::separator());
    // The Library lists the open chat's artifacts: that is their origin.
    items.push(group_menu("artifact", &id, &store().active_id.get_untracked()));
    items.push(CtxItem::separator());
    if a.archived {
        items.push(CtxItem::row("Unarchive", "eye", "lib-unhide", id.clone()));
    } else {
        items.push(CtxItem::row("Archive", "eyeOff", "lib-hide", id.clone()));
    }
    // Delete: the artifact leaves every chat, with all its versions, and
    // every group (a connected app's file stays where it lives: archive it)
    if app || a.kind == "image" && id.starts_with('R') {
        items.push(CtxItem::danger("Delete", "trash", "app-delete", id.clone()));
    } else if id.starts_with("file:") {
        items.push(CtxItem::danger("Delete", "trash", "lib-delete", id.clone()));
    }
    if app {
        if a.published {
            items.push(CtxItem::danger(
                "Delete Everywhere…",
                "trash",
                "app-delete-everywhere",
                id,
            ));
        }
    }
    items
}

/// `openCtx` — clamp the card inside the window like the TSX.
pub(crate) fn open(e: &ContextMenuEvent, items: Vec<CtxItem>) {
    open_at(e.page_position.x.0, e.page_position.y.0, items)
}

/// The same open at explicit page coords — the add-menu's measured
/// button position (App.tsx's `addMenu`).
pub(crate) fn open_at(x: f32, y: f32, items: Vec<CtxItem>) {
    open_near(x, y, y, items)
}

/// Opens the menu under `bottom`, or over `top` when it would run past
/// the window's bottom edge (as a native menu flips): a button's menu
/// passes its top and bottom, a point menu the point twice.
pub(crate) fn open_near(x: f32, top: f32, bottom: f32, items: Vec<CtxItem>) {
    let h = CTX_PAD * 2.0
        + items
            .iter()
            .map(|i| if i.sep { CTX_SEP } else { CTX_ROW })
            .sum::<f32>();
    let win = window();
    let (win_w, win_h) = (win.width.0, win.height.0);
    let x = (x - 4.0).min(win_w - CTX_W - 4.0).max(4.0);
    let y = if bottom - 4.0 + h <= win_h - 4.0 || top + 4.0 - h < 4.0 {
        (bottom - 4.0).min(win_h - h - 4.0)
    } else {
        top + 4.0 - h
    };
    let st = store();
    batch(|| {
        // a message's menu says so after opening (`ctx_owner`)
        st.ctx_owner.set(None);
        st.ctx_sub.set(-1);
        st.ctx_in_sub.set(false);
        st.ctx_sel.set(-1);
        st.ctx.set(Some(CtxMenu { x, y: y.max(4.0), items }));
    });
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
    let sub = store().ctx_sub;
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
            // card's right at the row's height (ctxSub in the TSX) — to
            // its left when the right side has no room, and lifted to
            // stay above the window's bottom edge.
            dynamic(move || {
                let i = sub2.get();
                if i < 0 {
                    return view().into_element();
                }
                let it = &items2[i as usize];
                if it.submenu.is_empty() {
                    return view().into_element();
                }
                let win = window();
                let (win_w, win_h) = (win.width.0, win.height.0);
                let right = menu.x + CTX_W - 8.0;
                let x = if right + CTX_W <= win_w - 4.0 { right } else { (menu.x - CTX_W + 8.0).max(4.0) };
                let h = row_top(&it.submenu, it.submenu.len()) + CTX_PAD;
                let y = (menu.y + row_top(&items2, i as usize) - CTX_PAD).min(win_h - h - 4.0).max(4.0);
                menu_card(x, y, it.submenu.clone(), sub_cancel.clone()).into_element()
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
            move |it, i| ctx_row_leaf(it.get(), i.get() as usize, cancel.clone()),
        ))
}

/// The group name prompt (`group_prompt`): a centered card with a name
/// input; OK makes the group (with the menu's item in it) or renames it.
pub(crate) fn prompt_layer() -> impl IntoElement {
    dynamic(|| {
        let Some(prompt) = store().group_prompt.get() else {
            return view().into_element();
        };
        let rename = !matches!(prompt, crate::store::GroupPrompt::New(_));
        let title = match &prompt {
            crate::store::GroupPrompt::New(_) => "New group",
            crate::store::GroupPrompt::Rename(_) => "Rename group",
            crate::store::GroupPrompt::RenameSection(_) => "Rename section",
        };
        let commit = move || {
            let st = store();
            // a group needs a name: the prompt stays open until it has one
            let name = st.group_name.get_untracked().trim().to_owned();
            if name.is_empty() {
                return;
            }
            match &prompt {
                crate::store::GroupPrompt::New(item) => st.new_group(&name, item.clone()),
                crate::store::GroupPrompt::Rename(gid) => {
                    st.update_group(gid, serde_json::json!({ "name": name }))
                }
                crate::store::GroupPrompt::RenameSection(key) => st.rename_section(key, &name),
            }
            st.group_prompt.set(None);
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
                    .absolute()
                    .bg(hsla(0.0, 0.0, 0.0, 0.4))
                    .on_press(|| store().group_prompt.set(None)),
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
                        text(title)
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT),
                    )
                    .child(
                        input()
                            .auto_focus(true)
                            .value(move || store().group_name.get())
                            .placeholder("Group name")
                            .placeholder_color(|| color(theme::GHOST))
                            .text_size(px(13.0))
                            .text_color(theme::TEXT)
                            .on_change(move |v: &str| store().group_name.set(v.to_owned()))
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
                                    .on_press(|| store().group_prompt.set(None)),
                            )
                            .child(
                                text(if rename { "Rename" } else { "Create" })
                                    .text_size(px(13.0))
                                    .text_color(white())
                                    .px_3()
                                    .py_1()
                                    .rounded(px(7.0))
                                    .bg(theme::ACCENT)
                                    .opacity(|| if store().group_name.get().trim().is_empty() { 0.4f32 } else { 1.0 })
                                    .on_press(move || commit()),
                            ),
                    ),
            )
            .into_element()
    })
}

/// `#rrggbb` → its color, for a well-formed hex only.
fn hex_color(s: &str) -> Option<Hsla> {
    let s = s.trim();
    let ok = s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit());
    ok.then(|| <Hsla as std::str::FromStr>::from_str(s).ok()).flatten()
}

/// HSL (h in degrees, s and l 0–1) → `#rrggbb`.
fn hsl_hex(h: f32, s: f32, l: f32) -> String {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}", to(r), to(g), to(b))
}

/// Color › Custom… (`color_edit`): a swatch grid — 12 hues across, light
/// to deep down, then a gray row — a `#rrggbb` field and a preview; Save
/// takes a well-formed hex only.
pub(crate) fn color_layer() -> impl IntoElement {
    dynamic(|| {
        let Some(gid) = store().color_edit.get() else {
            return view().into_element();
        };
        let save = move || {
            let st = store();
            let hex = st.color_draft.get_untracked().trim().to_lowercase();
            if hex_color(&hex).is_none() {
                return;
            }
            st.update_group(&gid, serde_json::json!({ "color": hex }));
            st.color_edit.set(None);
        };
        let save2 = save.clone();
        let mut rows: Vec<Vec<String>> = [0.88f32, 0.74, 0.6, 0.47, 0.34]
            .iter()
            .map(|l| (0..12).map(|i| hsl_hex(i as f32 * 30.0, 0.78, *l)).collect())
            .collect();
        rows.push((0..12).map(|i| hsl_hex(0.0, 0.0, 0.95 - i as f32 * 0.08)).collect());
        let grid = view().gap(px(4.0)).children(rows.into_iter().map(|row| {
            view()
                .flex_row()
                .gap(px(4.0))
                .children(row.into_iter().map(|hex| {
                    let c = hex_color(&hex).unwrap_or(hsla(0.0, 0.0, 0.5, 1.0));
                    let h2 = hex.clone();
                    view()
                        .size(px(20.0))
                        .rounded(px(5.0))
                        .bg(c)
                        .border(px(2.0))
                        .border_color(move || {
                            if store().color_draft.get().eq_ignore_ascii_case(&h2) { theme::TEXT } else { AdaptiveColor::TRANSPARENT }
                        })
                        .on_press(move || store().color_draft.set(hex.clone()))
                        .into_element()
                }))
                .into_element()
        }));
        view()
            .size_full()
            .absolute()
            .items_center()
            .justify_center()
            .child(
                view()
                    .size_full()
                    .absolute()
                    .bg(hsla(0.0, 0.0, 0.0, 0.4))
                    .on_press(|| store().color_edit.set(None)),
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
                        text("Group color")
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT),
                    )
                    .child(grid)
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                view()
                                    .size(px(24.0))
                                    .rounded(px(12.0))
                                    .border_1()
                                    .border_color(theme::BORDER_S)
                                    .bg(move || hex_color(&store().color_draft.get()).unwrap_or(hsla(0.0, 0.0, 0.0, 0.0))),
                            )
                            .child(
                                input()
                                    .auto_focus(true)
                                    .value(move || store().color_draft.get())
                                    .placeholder("#rrggbb")
                                    .placeholder_color(|| color(theme::GHOST))
                                    .text_size(px(13.0))
                                    .text_color(theme::TEXT)
                                    .on_change(move |v: &str| {
                                        let v = v.trim();
                                        let v = if v.is_empty() || v.starts_with('#') { v.to_owned() } else { format!("#{v}") };
                                        store().color_draft.set(v)
                                    })
                                    .on_submit(move |_: &str| save2())
                                    .flex_1()
                                    .h(px(30.0)),
                            ),
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
                                    .on_press(|| store().color_edit.set(None)),
                            )
                            .child(
                                text("Save")
                                    .text_size(px(13.0))
                                    .text_color(white())
                                    .px_3()
                                    .py_1()
                                    .rounded(px(7.0))
                                    .bg(theme::ACCENT)
                                    .opacity(|| if hex_color(&store().color_draft.get()).is_some() { 1.0f32 } else { 0.4 })
                                    .on_press(move || save()),
                            ),
                    ),
            )
            .into_element()
    })
}

/// A row's icon: a named glyph, or — for a `#rrggbb` "icon" (a group's
/// color) — a 10pt dot of that color.
fn row_icon(name: &str) -> AnyElement {
    match name.starts_with('#').then(|| <Hsla as std::str::FromStr>::from_str(name).ok()).flatten() {
        Some(c) => view().size(px(10.0)).rounded(px(5.0)).bg(c).into_element(),
        None => icon(name),
    }
}

/// A flyout row (no nested submenu — the TSX's sub-items are leaves).
/// `isSub` rows never touch `ctxSub`; their hover-in cancels a pending
/// close so the pointer can arrive from the parent row.
fn ctx_row_leaf(it: CtxItem, idx: usize, cancel: Rc<dyn Fn()>) -> impl IntoElement {
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
    let idx = idx as i64;
    let st = store();
    let keyed = move || st.ctx_in_sub.get() && st.ctx_sel.get() == idx;
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .h(px(CTX_ROW))
        .bg(move || {
            if hovered.get() || keyed() {
                theme::HOVER
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .child(view().w(px(16.0)).items_center().child(row_icon(&it.icon)))
        .child(text(it.label.clone()).text_size(px(13.0)).text_color(color))
        .child(view().flex_1())
        .when(it.checked, |v| {
            v.child(text("✓").text_size(px(12.0)).text_color(theme::ACCENT))
        })
        .child(key_hint(true, idx))
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
    let st = store();
    let keyed = move || !st.ctx_in_sub.get() && st.ctx_sel.get() == idx;
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .h(px(CTX_ROW))
        .bg(move || {
            if hovered.get() || keyed() || has_sub && sub.get() == idx {
                theme::HOVER
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .child(view().w(px(16.0)).items_center().child(row_icon(&it.icon)))
        .child(text(it.label.clone()).text_size(px(13.0)).text_color(color))
        .child(view().flex_1())
        .when(it.checked, |v| {
            v.child(text("✓").text_size(px(12.0)).text_color(theme::ACCENT))
        })
        .child(key_hint(false, idx))
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

// ——————————————— the open menu from the keyboard ———————————————

/// The rows the menu keys act in: the open flyout's while they are in it,
/// else the main card's.
fn keyed_items() -> Vec<CtxItem> {
    let st = store();
    let Some(menu) = st.ctx.get_untracked() else { return Vec::new() };
    let sub = st.ctx_sub.get_untracked();
    if st.ctx_in_sub.get_untracked() && sub >= 0 {
        menu.items.get(sub as usize).map(|i| i.submenu.clone()).unwrap_or_default()
    } else {
        menu.items
    }
}

/// The index of the `n`th (1-based) row that isn't a separator.
fn nth_row(items: &[CtxItem], n: usize) -> Option<usize> {
    items.iter().enumerate().filter(|(_, i)| !i.sep).nth(n.checked_sub(1)?).map(|(i, _)| i)
}

/// A row's number among its card's rows (1-based), separators skipped.
fn row_number(items: &[CtxItem], idx: usize) -> Option<usize> {
    (!items.get(idx)?.sep).then(|| items[..idx].iter().filter(|i| !i.sep).count() + 1)
}

/// The key that picks row `idx` of the main card (`in_sub` false) or the
/// flyout, shown while that card has the keys.
fn key_hint(in_sub: bool, idx: i64) -> impl IntoElement {
    dynamic(move || {
        let st = store();
        // the card the keys act in shows its keys (the main card until
        // Tab moves them into the flyout)
        if st.ctx_in_sub.get() != in_sub {
            return view().into_element();
        }
        let items = keyed_items();
        let Some(n) = row_number(&items, idx as usize).filter(|n| *n <= 9) else {
            return view().into_element();
        };
        let Some(c) = crate::keys::COMMANDS.iter().find(|c| c.id == format!("menu.pick_{n}")) else {
            return view().into_element();
        };
        let Some(k) = crate::keys::keys_of(c).into_iter().next() else { return view().into_element() };
        text(key_label(&k)).text_size(px(11.0)).text_color(theme::FAINT).into_element()
    })
}

/// ⌘1…⌘9: runs the nth row of the card the keys act in — a row with a
/// flyout opens it and moves the keys into it.
pub(crate) fn menu_pick(n: usize) {
    let st = store();
    let items = keyed_items();
    let Some(i) = nth_row(&items, n) else { return };
    open_or_run(i, &items);
}

fn open_or_run(i: usize, items: &[CtxItem]) {
    let st = store();
    let it = &items[i];
    if !it.submenu.is_empty() && !st.ctx_in_sub.get_untracked() {
        batch(|| {
            st.ctx_sub.set(i as i64);
            st.ctx_in_sub.set(true);
            st.ctx_sel.set(nth_row(&it.submenu, 1).map_or(-1, |x| x as i64));
        });
        return;
    }
    act(it);
}

/// ↑ / ↓: the highlight one row up or down (separators skipped, wraps).
pub(crate) fn menu_move(d: i64) {
    let st = store();
    let items = keyed_items();
    let rows: Vec<usize> = items.iter().enumerate().filter(|(_, i)| !i.sep).map(|(i, _)| i).collect();
    if rows.is_empty() {
        return;
    }
    let cur = st.ctx_sel.get_untracked();
    let at = rows.iter().position(|r| *r as i64 == cur);
    let next = match at {
        None if d > 0 => 0,
        None => rows.len() - 1,
        Some(p) => (p as i64 + d).rem_euclid(rows.len() as i64) as usize,
    };
    st.ctx_sel.set(rows[next] as i64);
}

/// ↩: runs the highlighted row (or opens its flyout).
pub(crate) fn menu_enter() {
    let st = store();
    let items = keyed_items();
    let sel = st.ctx_sel.get_untracked();
    if sel >= 0 && (sel as usize) < items.len() {
        open_or_run(sel as usize, &items);
    }
}

/// Tab / →: the keys into a flyout — the highlighted row's, the open
/// one, or the first row that has one.
pub(crate) fn menu_into_sub() {
    let st = store();
    if st.ctx_in_sub.get_untracked() {
        return;
    }
    let Some(menu) = st.ctx.get_untracked() else { return };
    let sel = st.ctx_sel.get_untracked();
    let open = st.ctx_sub.get_untracked();
    let pick = [sel, open]
        .into_iter()
        .find(|i| *i >= 0 && menu.items.get(*i as usize).is_some_and(|it| !it.submenu.is_empty()))
        .or_else(|| menu.items.iter().position(|it| !it.submenu.is_empty()).map(|i| i as i64));
    if let Some(i) = pick {
        open_or_run(i as usize, &menu.items);
    }
}

/// ⇧Tab / ←: the keys back to the main card, on the flyout's row.
pub(crate) fn menu_out_of_sub() {
    let st = store();
    if !st.ctx_in_sub.get_untracked() {
        return;
    }
    batch(|| {
        st.ctx_in_sub.set(false);
        st.ctx_sel.set(st.ctx_sub.get_untracked());
    });
}
