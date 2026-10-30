//! Keyboard commands. Every shortcut is a named command that lives in a
//! scope — the whole app, the chat, the palette, the file viewer, a modal
//! — with default keystrokes the user can change (Settings › Keyboard,
//! `keymap.json` through `GET/POST /keymap`; only the changes are kept).
//!
//! A key goes to the innermost open scope that binds it, then the scopes
//! around it, down to Global: one key can mean one thing in the viewer and
//! another in the chat. Only the keys some open scope binds are registered,
//! so the rest reach text fields and lists untouched.
//!
//! The menu bar's own shortcuts (Hide ⌘H, Hide Others ⌥⌘H, Minimize ⌘M,
//! the performance monitor's ⌘⇧F / ⌘⇧K) are commands too: while a command
//! keeps its menu key the menu item handles it; once the user moves it or
//! gives the key to another command, the item loses its key equivalent and
//! Rox binds the key itself.

use std::collections::{BTreeMap, BTreeSet};

use nsh_core::keymap::{Keystroke, When};
use nsh_core::prelude::*;

use crate::live::store;

/// Where a command applies, innermost first in [`ORDER`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub(crate) enum Scope {
    /// Settings › Keyboard is recording a key: nothing else binds.
    Recording,
    /// A right-click / ⋯ / ⌘K menu is open.
    Menu,
    Palette,
    /// A native spreadsheet's cell is being edited.
    SheetEdit,
    /// A native spreadsheet (`office/`), nothing being edited.
    Sheet,
    /// A native Word document (`office/docx/`), its find bar closed.
    Doc,
    /// The file viewer showing a spreadsheet.
    ViewerTable,
    Viewer,
    /// A server's mini app full screen.
    MiniApp,
    Lightbox,
    /// The plugins modal's search drop is open.
    PluginsSearch,
    Plugins,
    Settings,
    /// The composer's `/` and `@` popup is open over the chat.
    ComposerPopup,
    /// The chat with no modal over it.
    Chat,
    Global,
}

/// Innermost first: the first open scope binding a key takes it.
pub(crate) const ORDER: [Scope; 16] = [
    Scope::Recording,
    Scope::Menu,
    Scope::Palette,
    Scope::SheetEdit,
    Scope::Sheet,
    Scope::Doc,
    Scope::ViewerTable,
    Scope::Viewer,
    Scope::MiniApp,
    Scope::Lightbox,
    Scope::PluginsSearch,
    Scope::Plugins,
    Scope::Settings,
    Scope::ComposerPopup,
    Scope::Chat,
    Scope::Global,
];

impl Scope {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Scope::Recording => "$recording",
            Scope::Menu => "$menu",
            Scope::Palette => "$palette",
            Scope::SheetEdit => "$sheet-edit",
            Scope::Sheet => "$sheet",
            Scope::Doc => "$document",
            Scope::ViewerTable => "$spreadsheet",
            Scope::Viewer => "$viewer",
            Scope::MiniApp => "$mini-app",
            Scope::Lightbox => "$lightbox",
            Scope::PluginsSearch => "$plugins-search",
            Scope::Plugins => "$plugins",
            Scope::Settings => "$settings",
            Scope::ComposerPopup => "$composer-popup",
            Scope::Chat => "$chat",
            Scope::Global => "$global",
        }
    }

    /// Whether the scope is open (tracked: the bindings follow it).
    fn open(self) -> bool {
        let st = store();
        let viewing = || st.viewing.get();
        match self {
            Scope::Recording => st.key_record.get().is_some(),
            Scope::Menu => st.ctx.get().is_some(),
            Scope::Palette => st.palette.get(),
            Scope::SheetEdit => crate::office::xlsx::edit::editing(),
            Scope::Sheet => crate::office::xlsx::edit::navigating(),
            Scope::Doc => crate::office::docx::view::editing(),
            Scope::ViewerTable => viewing().is_some_and(|v| v.kind == "table") && !crate::office::showing(),
            Scope::Viewer => viewing().is_some(),
            Scope::MiniApp => !st.mini_full.with(Vec::is_empty),
            Scope::Lightbox => !st.light_items.get().is_empty(),
            Scope::PluginsSearch => st.plugins_open.get() && st.drop_open(),
            Scope::Plugins => st.plugins_open.get(),
            Scope::Settings => st.settings_open.get(),
            Scope::ComposerPopup => Scope::Chat.open() && !crate::slash::rows().is_empty(),
            Scope::Chat => {
                !st.palette.get()
                    && viewing().is_none()
                    && st.mini_full.with(Vec::is_empty)
                    && st.light_items.get().is_empty()
                    && !st.plugins_open.get()
                    && !st.settings_open.get()
                    && st.bot_details.get().is_none()
                    && st.app_details.get().is_none()
                    && !st.bot_modal.get()
            }
            Scope::Global => true,
        }
    }
}

/// A menu bar item that owns a command's default key until the user
/// moves it: its action and the key it shows.
#[derive(Clone, Copy)]
pub(crate) struct MenuKey {
    pub action: &'static str,
    pub key: &'static str,
}

pub(crate) struct Command {
    pub id: &'static str,
    pub title: &'static str,
    pub scope: Scope,
    pub keys: &'static [&'static str],
    pub run: fn(u8),
    /// Passed to `run` (mod+1…9 share one function).
    pub arg: u8,
    /// Fires on a held key's repeats.
    pub repeat: bool,
    /// While a text field has the focus: by default not for a key the
    /// field uses itself (arrows, Escape, ⌘A/⌘C/⌘V/⌘X/⌘Y/⌘Z…).
    pub when: When,
    /// The menu item that handles the default key itself.
    pub menu: Option<MenuKey>,
}

const fn cmd(id: &'static str, title: &'static str, scope: Scope, keys: &'static [&'static str], run: fn(u8)) -> Command {
    Command { id, title, scope, keys, run, arg: 0, repeat: false, when: When::Auto, menu: None }
}

/// Fires in text fields too.
const fn always(c: Command) -> Command {
    Command { when: When::Always, ..c }
}

const fn repeating(c: Command) -> Command {
    Command { repeat: true, ..c }
}

const fn menu(c: Command, action: &'static str, key: &'static str) -> Command {
    Command { menu: Some(MenuKey { action, key }), ..c }
}

const fn with_arg(c: Command, arg: u8) -> Command {
    Command { arg, ..c }
}

/// Every command, in the order Settings › Keyboard lists them.
pub(crate) static COMMANDS: &[Command] = &[
    // ——— Everywhere ———
    cmd("palette.toggle", "Command palette", Scope::Global, &["mod+k"], |_| store().toggle_palette()),
    cmd("settings.open", "Settings", Scope::Global, &["mod+,"], |_| crate::settings::toggle()),
    cmd("history.back", "Back", Scope::Global, &["mod+["], |_| crate::history::step(-1)),
    cmd("history.forward", "Forward", Scope::Global, &["mod+]"], |_| crate::history::step(1)),
    cmd("composer.focus", "Focus the message field", Scope::Global, &["mod+l"], |_| crate::composer::focus()),
    always(cmd("app.escape", "Close / leave the field", Scope::Global, &["escape"], |_| escape())),
    cmd("chat.new", "New chat", Scope::Global, &["mod+shift+n"], |_| store().new_thread()),
    cmd("bot.new", "New bot", Scope::Global, &["mod+n"], |_| store().bot_modal.set(true)),
    cmd("sidebar.toggle", "Show / hide the sidebar", Scope::Global, &["mod+b"], |_| {
        store().side_shown.update(|v| *v = !*v)
    }),
    cmd("sidebar.mode", "Sidebar: full / rail", Scope::Global, &["mod+shift+h"], |_| {
        let st = store();
        st.side_shown.set(true);
        st.side_mode.set(if st.side_mode.get_untracked() == "full" { "rail" } else { "full" }.to_owned());
    }),
    cmd("pane.toggle", "Show / hide the details pane", Scope::Global, &["mod+shift+b"], |_| {
        store().right_open.update(|v| *v = !*v)
    }),
    cmd("theme.next", "Next theme", Scope::Global, &["mod+t"], |_| {
        let st = store();
        st.set_theme(st.next_theme());
    }),
    cmd("view.zoom_in", "Zoom in", Scope::Global, &["mod+plus", "mod+shift+plus"], |_| store().zoom_step(1)),
    cmd("view.zoom_out", "Zoom out", Scope::Global, &["mod+minus"], |_| store().zoom_step(-1)),
    cmd("view.zoom_reset", "Actual size", Scope::Global, &["mod+0"], |_| store().zoom_step(0)),
    cmd("app.reload", "Reload", Scope::Global, &["mod+r"], |_| store().reset_fresh()),
    cmd("voice.dictate", "Dictate", Scope::Global, &["mod+d"], |_| store().dictate_stub()),
    with_arg(cmd("chat.open_1", "Open sidebar chat 1", Scope::Global, &["mod+1"], open_slot), 1),
    with_arg(cmd("chat.open_2", "Open sidebar chat 2", Scope::Global, &["mod+2"], open_slot), 2),
    with_arg(cmd("chat.open_3", "Open sidebar chat 3", Scope::Global, &["mod+3"], open_slot), 3),
    with_arg(cmd("chat.open_4", "Open sidebar chat 4", Scope::Global, &["mod+4"], open_slot), 4),
    with_arg(cmd("chat.open_5", "Open sidebar chat 5", Scope::Global, &["mod+5"], open_slot), 5),
    with_arg(cmd("chat.open_6", "Open sidebar chat 6", Scope::Global, &["mod+6"], open_slot), 6),
    with_arg(cmd("chat.open_7", "Open sidebar chat 7", Scope::Global, &["mod+7"], open_slot), 7),
    with_arg(cmd("chat.open_8", "Open sidebar chat 8", Scope::Global, &["mod+8"], open_slot), 8),
    with_arg(cmd("chat.open_9", "Open sidebar chat 9", Scope::Global, &["mod+9"], open_slot), 9),
    menu(cmd("app.hide", "Hide Rox", Scope::Global, &["mod+h"], |_| send_action("hide:")), "hide:", "mod+h"),
    menu(
        cmd("app.hide_others", "Hide others", Scope::Global, &["mod+alt+h"], |_| send_action("hideOtherApplications:")),
        "hideOtherApplications:",
        "mod+alt+h",
    ),
    menu(
        cmd("window.minimize", "Minimize", Scope::Global, &["mod+m"], |_| send_action("performMiniaturize:")),
        "performMiniaturize:",
        "mod+m",
    ),
    cmd("dev.livebar", "ra-jit stats pill", Scope::Global, &["mod+shift+r"], |_| {
        if let Some(show) = store().livebar.get() {
            show.update(|v| *v = !*v);
        }
    }),
    menu(
        cmd("dev.perf", "Performance monitor", Scope::Global, &["mod+shift+f"], |_| send_action("toggleFpsMonitor:")),
        "toggleFpsMonitor:",
        "mod+shift+f",
    ),
    menu(
        cmd("dev.perf_clear", "Clear the performance monitor", Scope::Global, &["mod+shift+k"], |_| {
            send_action("clearFpsMonitor:")
        }),
        "clearFpsMonitor:",
        "mod+shift+k",
    ),
    // ——— Chat ———
    repeating(cmd("chat.prev_msg", "Previous message", Scope::Chat, &["mod+up"], |_| step_msg(-1))),
    repeating(cmd("chat.next_msg", "Next message", Scope::Chat, &["mod+down"], |_| step_msg(1))),
    cmd("chat.msg_menu", "Message menu", Scope::Chat, &["mod+u"], |_| msg_menu()),
    cmd("chat.reply", "Reply to the message", Scope::Chat, &["mod+y"], |_| reply()),
    always(cmd("composer.clear", "Clear the message field (text, files, reply)", Scope::Global, &["mod+shift+backspace"], |_| clear_composer())),
    // ——— The composer's / and @ popup ———
    always(repeating(cmd("popup.up", "Previous suggestion", Scope::ComposerPopup, &["up"], |_| crate::slash::step(-1)))),
    always(repeating(cmd("popup.down", "Next suggestion", Scope::ComposerPopup, &["down"], |_| crate::slash::step(1)))),
    always(cmd("popup.pick", "Pick the suggestion", Scope::ComposerPopup, &["enter"], |_| crate::slash::enter())),
    always(cmd("popup.complete", "Complete with the suggestion", Scope::ComposerPopup, &["tab"], |_| crate::slash::tab())),
    always(cmd("popup.close", "Close the suggestions", Scope::ComposerPopup, &["escape"], |_| crate::slash::close())),
    always(with_arg(cmd("popup.pick_1", "Pick suggestion 1", Scope::ComposerPopup, &["mod+1"], popup_pick), 1)),
    always(with_arg(cmd("popup.pick_2", "Pick suggestion 2", Scope::ComposerPopup, &["mod+2"], popup_pick), 2)),
    always(with_arg(cmd("popup.pick_3", "Pick suggestion 3", Scope::ComposerPopup, &["mod+3"], popup_pick), 3)),
    always(with_arg(cmd("popup.pick_4", "Pick suggestion 4", Scope::ComposerPopup, &["mod+4"], popup_pick), 4)),
    always(with_arg(cmd("popup.pick_5", "Pick suggestion 5", Scope::ComposerPopup, &["mod+5"], popup_pick), 5)),
    always(with_arg(cmd("popup.pick_6", "Pick suggestion 6", Scope::ComposerPopup, &["mod+6"], popup_pick), 6)),
    always(with_arg(cmd("popup.pick_7", "Pick suggestion 7", Scope::ComposerPopup, &["mod+7"], popup_pick), 7)),
    always(with_arg(cmd("popup.pick_8", "Pick suggestion 8", Scope::ComposerPopup, &["mod+8"], popup_pick), 8)),
    always(with_arg(cmd("popup.pick_9", "Pick suggestion 9", Scope::ComposerPopup, &["mod+9"], popup_pick), 9)),
    // ——— Menus (right-click, ⋯, the viewer's ⌘K) ———
    with_arg(cmd("menu.pick_1", "Pick menu item 1", Scope::Menu, &["mod+1"], menu_pick), 1),
    with_arg(cmd("menu.pick_2", "Pick menu item 2", Scope::Menu, &["mod+2"], menu_pick), 2),
    with_arg(cmd("menu.pick_3", "Pick menu item 3", Scope::Menu, &["mod+3"], menu_pick), 3),
    with_arg(cmd("menu.pick_4", "Pick menu item 4", Scope::Menu, &["mod+4"], menu_pick), 4),
    with_arg(cmd("menu.pick_5", "Pick menu item 5", Scope::Menu, &["mod+5"], menu_pick), 5),
    with_arg(cmd("menu.pick_6", "Pick menu item 6", Scope::Menu, &["mod+6"], menu_pick), 6),
    with_arg(cmd("menu.pick_7", "Pick menu item 7", Scope::Menu, &["mod+7"], menu_pick), 7),
    with_arg(cmd("menu.pick_8", "Pick menu item 8", Scope::Menu, &["mod+8"], menu_pick), 8),
    with_arg(cmd("menu.pick_9", "Pick menu item 9", Scope::Menu, &["mod+9"], menu_pick), 9),
    always(repeating(cmd("menu.up", "Previous item", Scope::Menu, &["up"], |_| crate::ctx::menu_move(-1)))),
    always(repeating(cmd("menu.down", "Next item", Scope::Menu, &["down"], |_| crate::ctx::menu_move(1)))),
    always(cmd("menu.run", "Run the item", Scope::Menu, &["enter"], |_| crate::ctx::menu_enter())),
    always(cmd("menu.into_sub", "Into the submenu", Scope::Menu, &["tab", "right"], |_| crate::ctx::menu_into_sub())),
    always(cmd("menu.out_of_sub", "Back to the menu", Scope::Menu, &["shift+tab", "left"], |_| crate::ctx::menu_out_of_sub())),
    always(cmd("menu.close", "Close the menu", Scope::Menu, &["escape"], |_| store().ctx.set(None))),
    // ——— Command palette ———
    repeating(cmd("palette.up", "Previous result", Scope::Palette, &["up"], |_| palette_step(-1))),
    repeating(cmd("palette.down", "Next result", Scope::Palette, &["down"], |_| palette_step(1))),
    cmd("palette.run", "Run the result", Scope::Palette, &["enter"], |_| {
        let st = store();
        st.palette_run_at(st.palette_sel.get_untracked() as usize);
    }),
    with_arg(cmd("palette.run_1", "Run result 1", Scope::Palette, &["mod+1"], palette_run), 1),
    with_arg(cmd("palette.run_2", "Run result 2", Scope::Palette, &["mod+2"], palette_run), 2),
    with_arg(cmd("palette.run_3", "Run result 3", Scope::Palette, &["mod+3"], palette_run), 3),
    with_arg(cmd("palette.run_4", "Run result 4", Scope::Palette, &["mod+4"], palette_run), 4),
    with_arg(cmd("palette.run_5", "Run result 5", Scope::Palette, &["mod+5"], palette_run), 5),
    with_arg(cmd("palette.run_6", "Run result 6", Scope::Palette, &["mod+6"], palette_run), 6),
    with_arg(cmd("palette.run_7", "Run result 7", Scope::Palette, &["mod+7"], palette_run), 7),
    with_arg(cmd("palette.run_8", "Run result 8", Scope::Palette, &["mod+8"], palette_run), 8),
    with_arg(cmd("palette.run_9", "Run result 9", Scope::Palette, &["mod+9"], palette_run), 9),
    // ——— Settings ———
    always(repeating(cmd("settings.up", "Previous shortcut / suggestion", Scope::Settings, &["up"], |_| crate::settings::step(-1)))),
    always(repeating(cmd("settings.down", "Next shortcut / suggestion", Scope::Settings, &["down"], |_| crate::settings::step(1)))),
    always(cmd("settings.enter", "Pick the suggestion / change the shortcut", Scope::Settings, &["enter"], |_| crate::settings::enter())),
    // ——— File viewer ———
    cmd("viewer.actions", "File actions", Scope::Viewer, &["mod+k"], |_| crate::files::viewer_actions()),
    // ⌘W, not Escape: Escape leaves the app's own fields mid-filing. The
    // viewer over the apps closes first, then the top app.
    cmd("mini.close", "Close the mini app", Scope::MiniApp, &["mod+w"], |_| store().mini_close_top()),
    cmd("viewer.print", "Print", Scope::Viewer, &["mod+p"], |_| crate::files::viewer_print()),
    cmd("viewer.save", "Save as a new version (Office)", Scope::Viewer, &["mod+s"], |_| crate::files::viewer_save()),
    cmd("viewer.download", "Download", Scope::Viewer, &["mod+shift+s"], |_| {
        if let Some(f) = store().viewing.get_untracked() {
            store().save_file(&f.path);
        }
    }),
    cmd("viewer.version_prev", "Older version", Scope::Viewer, &["mod+shift+["], |_| crate::files::viewer_version_step(-1)),
    cmd("viewer.version_next", "Newer version", Scope::Viewer, &["mod+shift+]"], |_| crate::files::viewer_version_step(1)),
    repeating(cmd("table.left", "Cell left", Scope::ViewerTable, &["left"], |_| store().sel_move(0, -1, false))),
    repeating(cmd("table.right", "Cell right", Scope::ViewerTable, &["right"], |_| store().sel_move(0, 1, false))),
    repeating(cmd("table.up", "Cell up", Scope::ViewerTable, &["up"], |_| store().sel_move(-1, 0, false))),
    repeating(cmd("table.down", "Cell down", Scope::ViewerTable, &["down"], |_| store().sel_move(1, 0, false))),
    repeating(cmd("table.extend_left", "Extend left", Scope::ViewerTable, &["shift+left"], |_| store().sel_move(0, -1, true))),
    repeating(cmd("table.extend_right", "Extend right", Scope::ViewerTable, &["shift+right"], |_| store().sel_move(0, 1, true))),
    repeating(cmd("table.extend_up", "Extend up", Scope::ViewerTable, &["shift+up"], |_| store().sel_move(-1, 0, true))),
    repeating(cmd("table.extend_down", "Extend down", Scope::ViewerTable, &["shift+down"], |_| store().sel_move(1, 0, true))),
    cmd("table.copy", "Copy the cells", Scope::ViewerTable, &["mod+c"], |_| {
        let st = store();
        if st.sel_b.get_untracked().is_some() {
            st.copy_sel();
        }
    }),
    // ——— Native spreadsheet (`office/xlsx`) ———
    // Bound over the cell editor, which keeps the focus to catch typing.
    always(repeating(cmd("sheet.up", "Cell up", Scope::Sheet, &["up"], |_| sheet_nav(Dir::Up, Step::Cell, false)))),
    always(repeating(cmd("sheet.down", "Cell down", Scope::Sheet, &["down"], |_| sheet_nav(Dir::Down, Step::Cell, false)))),
    always(repeating(cmd("sheet.left", "Cell left", Scope::Sheet, &["left"], |_| sheet_nav(Dir::Left, Step::Cell, false)))),
    always(repeating(cmd("sheet.right", "Cell right", Scope::Sheet, &["right"], |_| sheet_nav(Dir::Right, Step::Cell, false)))),
    always(repeating(cmd("sheet.extend_up", "Extend up", Scope::Sheet, &["shift+up"], |_| sheet_nav(Dir::Up, Step::Cell, true)))),
    always(repeating(cmd("sheet.extend_down", "Extend down", Scope::Sheet, &["shift+down"], |_| sheet_nav(Dir::Down, Step::Cell, true)))),
    always(repeating(cmd("sheet.extend_left", "Extend left", Scope::Sheet, &["shift+left"], |_| sheet_nav(Dir::Left, Step::Cell, true)))),
    always(repeating(cmd("sheet.extend_right", "Extend right", Scope::Sheet, &["shift+right"], |_| sheet_nav(Dir::Right, Step::Cell, true)))),
    always(cmd("sheet.edge_up", "To the top of the data", Scope::Sheet, &["mod+up"], |_| sheet_nav(Dir::Up, Step::Edge, false))),
    always(cmd("sheet.edge_down", "To the bottom of the data", Scope::Sheet, &["mod+down"], |_| sheet_nav(Dir::Down, Step::Edge, false))),
    always(cmd("sheet.edge_left", "To the first column", Scope::Sheet, &["mod+left"], |_| sheet_nav(Dir::Left, Step::Edge, false))),
    always(cmd("sheet.edge_right", "To the last column of the data", Scope::Sheet, &["mod+right"], |_| sheet_nav(Dir::Right, Step::Edge, false))),
    always(cmd("sheet.extend_edge_up", "Extend to the top", Scope::Sheet, &["mod+shift+up"], |_| sheet_nav(Dir::Up, Step::Edge, true))),
    always(cmd("sheet.extend_edge_down", "Extend to the bottom of the data", Scope::Sheet, &["mod+shift+down"], |_| sheet_nav(Dir::Down, Step::Edge, true))),
    always(cmd("sheet.extend_edge_left", "Extend to the first column", Scope::Sheet, &["mod+shift+left"], |_| sheet_nav(Dir::Left, Step::Edge, true))),
    always(cmd("sheet.extend_edge_right", "Extend to the last column of the data", Scope::Sheet, &["mod+shift+right"], |_| sheet_nav(Dir::Right, Step::Edge, true))),
    always(repeating(cmd("sheet.page_up", "Page up", Scope::Sheet, &["pageup"], |_| sheet_nav(Dir::Up, Step::Page, false)))),
    always(repeating(cmd("sheet.page_down", "Page down", Scope::Sheet, &["pagedown"], |_| sheet_nav(Dir::Down, Step::Page, false)))),
    always(repeating(cmd("sheet.next", "Next cell", Scope::Sheet, &["tab"], |_| crate::office::xlsx::edit::walk(Dir::Right)))),
    always(repeating(cmd("sheet.prev", "Previous cell", Scope::Sheet, &["shift+tab"], |_| crate::office::xlsx::edit::walk(Dir::Left)))),
    always(repeating(cmd("sheet.enter", "Cell below", Scope::Sheet, &["enter"], |_| crate::office::xlsx::edit::walk(Dir::Down)))),
    always(repeating(cmd("sheet.enter_up", "Cell above", Scope::Sheet, &["shift+enter"], |_| crate::office::xlsx::edit::walk(Dir::Up)))),
    always(cmd("sheet.edit", "Edit the cell", Scope::Sheet, &["f2"], |_| crate::office::xlsx::edit::edit_active())),
    always(cmd("sheet.all", "Select all", Scope::Sheet, &["mod+a"], |_| crate::office::xlsx::edit::select_all())),
    always(cmd("sheet.copy", "Copy", Scope::Sheet, &["mod+c"], |_| crate::office::xlsx::edit::copy(false))),
    always(cmd("sheet.cut", "Cut", Scope::Sheet, &["mod+x"], |_| crate::office::xlsx::edit::copy(true))),
    always(cmd("sheet.paste", "Paste", Scope::Sheet, &["mod+v"], |_| crate::office::xlsx::edit::paste())),
    always(cmd("sheet.clear", "Clear the cells", Scope::Sheet, &["delete", "backspace"], |_| crate::office::xlsx::edit::clear())),
    always(repeating(cmd("sheet.undo", "Undo", Scope::Sheet, &["mod+z"], |_| crate::office::xlsx::edit::undo(false)))),
    always(repeating(cmd("sheet.redo", "Redo", Scope::Sheet, &["mod+shift+z", "mod+y"], |_| crate::office::xlsx::edit::undo(true)))),
    always(cmd("sheet.escape", "Collapse the selection / close", Scope::Sheet, &["escape"], |_| crate::office::xlsx::edit::escape())),
    always(cmd("sheet_edit.next", "Enter and the next cell", Scope::SheetEdit, &["tab"], |_| crate::office::xlsx::edit::commit_and(Dir::Right))),
    always(cmd("sheet_edit.prev", "Enter and the previous cell", Scope::SheetEdit, &["shift+tab"], |_| crate::office::xlsx::edit::commit_and(Dir::Left))),
    always(cmd("sheet_edit.up", "Enter and the cell above", Scope::SheetEdit, &["shift+enter"], |_| crate::office::xlsx::edit::commit_and(Dir::Up))),
    always(cmd("sheet_edit.cancel", "Cancel the edit", Scope::SheetEdit, &["escape"], |_| crate::office::xlsx::edit::cancel())),
    // ——— Word (`office/docx/view.rs`) ———
    always(repeating(cmd("doc.left", "Left", Scope::Doc, &["left"], |_| doc_step(Move::Left, false)))),
    always(repeating(cmd("doc.right", "Right", Scope::Doc, &["right"], |_| doc_step(Move::Right, false)))),
    always(repeating(cmd("doc.up", "Line up", Scope::Doc, &["up"], |_| doc_step(Move::Up, false)))),
    always(repeating(cmd("doc.down", "Line down", Scope::Doc, &["down"], |_| doc_step(Move::Down, false)))),
    always(repeating(cmd("doc.extend_left", "Extend left", Scope::Doc, &["shift+left"], |_| doc_step(Move::Left, true)))),
    always(repeating(cmd("doc.extend_right", "Extend right", Scope::Doc, &["shift+right"], |_| doc_step(Move::Right, true)))),
    always(repeating(cmd("doc.extend_up", "Extend up", Scope::Doc, &["shift+up"], |_| doc_step(Move::Up, true)))),
    always(repeating(cmd("doc.extend_down", "Extend down", Scope::Doc, &["shift+down"], |_| doc_step(Move::Down, true)))),
    always(repeating(cmd("doc.word_left", "Word left", Scope::Doc, &["alt+left"], |_| doc_step(Move::WordLeft, false)))),
    always(repeating(cmd("doc.word_right", "Word right", Scope::Doc, &["alt+right"], |_| doc_step(Move::WordRight, false)))),
    always(repeating(cmd("doc.extend_word_left", "Extend a word left", Scope::Doc, &["alt+shift+left"], |_| doc_step(Move::WordLeft, true)))),
    always(repeating(cmd("doc.extend_word_right", "Extend a word right", Scope::Doc, &["alt+shift+right"], |_| doc_step(Move::WordRight, true)))),
    always(cmd("doc.line_start", "Line start", Scope::Doc, &["mod+left", "home"], |_| doc_step(Move::LineStart, false))),
    always(cmd("doc.line_end", "Line end", Scope::Doc, &["mod+right", "end"], |_| doc_step(Move::LineEnd, false))),
    always(cmd("doc.extend_line_start", "Extend to the line start", Scope::Doc, &["mod+shift+left", "shift+home"], |_| doc_step(Move::LineStart, true))),
    always(cmd("doc.extend_line_end", "Extend to the line end", Scope::Doc, &["mod+shift+right", "shift+end"], |_| doc_step(Move::LineEnd, true))),
    always(cmd("doc.start", "Document start", Scope::Doc, &["mod+up"], |_| doc_step(Move::DocStart, false))),
    always(cmd("doc.end", "Document end", Scope::Doc, &["mod+down"], |_| doc_step(Move::DocEnd, false))),
    always(cmd("doc.extend_start", "Extend to the start", Scope::Doc, &["mod+shift+up"], |_| doc_step(Move::DocStart, true))),
    always(cmd("doc.extend_end", "Extend to the end", Scope::Doc, &["mod+shift+down"], |_| doc_step(Move::DocEnd, true))),
    always(repeating(cmd("doc.page_up", "Page up", Scope::Doc, &["pageup"], |_| doc_step(Move::PageUp, false)))),
    always(repeating(cmd("doc.page_down", "Page down", Scope::Doc, &["pagedown"], |_| doc_step(Move::PageDown, false)))),
    always(repeating(cmd("doc.backspace", "Delete back", Scope::Doc, &["backspace"], |_| crate::office::docx::view::key("backspace")))),
    always(repeating(cmd("doc.delete", "Delete forward", Scope::Doc, &["delete"], |_| crate::office::docx::view::key("delete")))),
    always(repeating(cmd("doc.enter", "New paragraph", Scope::Doc, &["enter"], |_| crate::office::docx::view::key("enter")))),
    always(repeating(cmd("doc.tab", "Tab", Scope::Doc, &["tab"], |_| crate::office::docx::view::key("tab")))),
    always(cmd("doc.all", "Select all", Scope::Doc, &["mod+a"], |_| crate::office::docx::view::key("all"))),
    always(cmd("doc.copy", "Copy", Scope::Doc, &["mod+c"], |_| crate::office::docx::view::copy(false))),
    always(cmd("doc.cut", "Cut", Scope::Doc, &["mod+x"], |_| crate::office::docx::view::copy(true))),
    always(cmd("doc.paste", "Paste", Scope::Doc, &["mod+v"], |_| crate::office::docx::view::paste())),
    always(repeating(cmd("doc.undo", "Undo", Scope::Doc, &["mod+z"], |_| crate::office::docx::view::key("undo")))),
    always(repeating(cmd("doc.redo", "Redo", Scope::Doc, &["mod+shift+z", "mod+y"], |_| crate::office::docx::view::key("redo")))),
    always(cmd("doc.bold", "Bold", Scope::Doc, &["mod+b"], |_| doc_format(Fmt::Bold))),
    always(cmd("doc.italic", "Italic", Scope::Doc, &["mod+i"], |_| doc_format(Fmt::Italic))),
    always(cmd("doc.underline", "Underline", Scope::Doc, &["mod+u"], |_| doc_format(Fmt::Underline))),
    always(cmd("doc.bigger", "Bigger text", Scope::Doc, &["mod+shift+."], |_| doc_format(Fmt::Size(true)))),
    always(cmd("doc.smaller", "Smaller text", Scope::Doc, &["mod+shift+,"], |_| doc_format(Fmt::Size(false)))),
    always(cmd("doc.bullets", "Bulleted list", Scope::Doc, &["mod+shift+8"], |_| doc_format(Fmt::Bullets))),
    always(cmd("doc.numbers", "Numbered list", Scope::Doc, &["mod+shift+7"], |_| doc_format(Fmt::Numbers))),
    always(cmd("doc.find", "Find", Scope::Doc, &["mod+f"], |_| crate::office::docx::view::find())),
    always(cmd("doc.zoom_in", "Zoom in", Scope::Doc, &["mod+="], |_| crate::office::docx::view::zoom_by(0.1))),
    always(cmd("doc.zoom_out", "Zoom out", Scope::Doc, &["mod+-"], |_| crate::office::docx::view::zoom_by(-0.1))),
    always(cmd("doc.escape", "Close", Scope::Doc, &["escape"], |_| crate::office::docx::view::escape())),
    // ——— Lightbox ———
    repeating(cmd("light.prev", "Previous image", Scope::Lightbox, &["left"], |_| store().light_step(-1))),
    repeating(cmd("light.next", "Next image", Scope::Lightbox, &["right"], |_| store().light_step(1))),
    // ——— Plugins search ———
    repeating(cmd("plugins.up", "Previous match", Scope::PluginsSearch, &["up"], |_| drop_step(-1))),
    repeating(cmd("plugins.down", "Next match", Scope::PluginsSearch, &["down"], |_| drop_step(1))),
    cmd("plugins.choose", "Open the match", Scope::PluginsSearch, &["enter"], |_| store().drop_choose()),
];

use crate::office::docx::doc::{Fmt, Move};
use crate::office::xlsx::select::{Dir, Step};

fn doc_step(to: Move, extend: bool) {
    crate::office::docx::view::step(to, extend);
}

fn doc_format(f: Fmt) {
    crate::office::docx::view::format(f);
}

fn sheet_nav(dir: Dir, step: Step, shift: bool) {
    crate::office::xlsx::edit::nav(dir, step, shift);
}

fn menu_pick(n: u8) {
    crate::ctx::menu_pick(n as usize);
}

fn popup_pick(n: u8) {
    crate::slash::pick_shown(n as usize);
}

fn open_slot(k: u8) {
    let st = store();
    if let Some(id) = st.shortcut_threads().get((k - 1) as usize) {
        st.open_thread(id.clone());
    }
}

fn palette_run(k: u8) {
    let st = store();
    if !st.palette_q.get_untracked().trim().is_empty() {
        st.palette_run_at((k - 1) as usize);
    }
}

fn palette_step(d: i64) {
    let st = store();
    let n = st.palette_results().len() as i64;
    if n > 0 {
        st.palette_sel.update(|i| *i = (*i + n + d) % n);
    }
}

fn drop_step(d: i64) {
    let st = store();
    let n = st.plugin_list().len();
    if n > 0 {
        st.drop_sel.update(|i| *i = ((*i as i64 + n as i64 + d) % n as i64) as usize);
    }
}

/// Escape: closes the topmost things (palette, modals, viewer, …) as it
/// always has; with nothing open, drops the message selection and leaves
/// the message field.
fn escape() {
    let st = store();
    let anything = st.palette.get_untracked()
        || st.bot_modal.get_untracked()
        || st.vers_pop.get_untracked().is_some()
        || st.app_details.get_untracked().is_some()
        || st.main_pick.get_untracked().is_some()
        || st.link_ask.get_untracked().is_some()
        || !st.light_items.get_untracked().is_empty()
        || st.viewing.get_untracked().is_some()
        || st.plugins_open.get_untracked()
        || st.settings_open.get_untracked()
        || st.bot_details.get_untracked().is_some();
    if st.palette.get_untracked() {
        st.close_palette();
    }
    st.bot_modal.set(false);
    st.vers_pop.set(None);
    st.app_details.set(None);
    st.main_pick.set(None);
    // a waiting question hears the no (a v1 app's ask, 9.7)
    st.answer_link_ask(false);
    st.avatar_editing.set(false);
    // lightbox + fullscreen viewer (lightbox.tsx:98, fileviewer.tsx:783)
    st.light_close();
    if crate::office::may_close() {
        st.viewing.set(None);
    }
    crate::models::close();
    crate::composer::plus_menu().set(None);
    // the plugins modal pops a page, or closes at the market page.
    if st.plugins_open.get_untracked() {
        if st.plugin_nav.get_untracked().len() > 1 {
            st.pop_nav();
        } else {
            st.close_plugins();
        }
    }
    st.settings_open.set(false);
    // Bot Details steps back a page, or closes on its first
    if st.bot_details.get_untracked().is_some() {
        if st.bd_nav.get_untracked().len() > 1 {
            st.bd_nav.update(|n| {
                n.pop();
            });
        } else {
            st.bot_details.set(None);
        }
    }
    if !anything {
        // the position stays (the next ⌘↑ / ⌘↓ starts there); its ring hides
        st.sel_shown.set(false);
        dismiss_keyboard();
    }
}

/// ⌘↑ / ⌘↓: walks the open chat's messages. The first press shows the
/// position kept for the chat (where the keys left it, a link's message,
/// or the bottom message of the last scroll) — the newest without one —
/// and the next ones move it.
fn step_msg(d: i64) {
    step_msg_with(d, true);
}

/// [`step_msg`], the scroll animated or not (a menu about to open on the
/// message needs it where it ends up).
fn step_msg_with(d: i64, animated: bool) {
    let st = store();
    let turns = st.turns.get_untracked();
    if turns.is_empty() {
        return;
    }
    let at = st.sel_msg.get_untracked().and_then(|i| turns.iter().position(|t| t.i == i));
    let next = match at {
        None => turns.len() - 1,
        Some(p) if !st.sel_shown.get_untracked() => p,
        Some(p) => (p as i64 + d).clamp(0, turns.len() as i64 - 1) as usize,
    };
    st.sel_stepped.set(Some(std::time::Instant::now()));
    // a menu open on the old message goes with it
    st.ctx.set(None);
    st.set_sel(turns[next].i, true);
    if let Some(list) = st.chat_list.borrow().as_ref() {
        list.scroll_to_index(next as u32, percentage(0.5), px(0.0), animated);
    }
}

/// The open chat's kept position, restored on every chat switch (hidden).
pub(crate) fn track_sel() {
    effect(|| {
        let st = store();
        let chat = st.active_id.get();
        untracked(|| {
            let i = st.sel_marks.borrow().get(&chat).copied();
            batch(|| {
                st.sel_msg.set(i);
                st.sel_shown.set(false);
            });
        });
    });
}

/// ⌘⇧⌫: empties the message field — its text, attachments and reply.
fn clear_composer() {
    let st = store();
    batch(|| {
        st.draft.set(String::new());
        st.attachments.set(Vec::new());
        st.reply_to.set(None);
    });
}

/// ⌘U: the selected message's ⋯ menu (a card's actions for a card),
/// opened where its ⋯ is — under the message, at its left (a reply) or
/// right (yours) edge.
fn msg_menu() {
    let st = store();
    // the message moves into view first, at once — the menu then opens
    // where it ends up, not where it was
    step_msg_with(0, false);
    let Some(i) = st.sel_msg.get_untracked() else { return };
    let row = st.turns.get_untracked().into_iter().find(|t| t.i == i);
    let Some(row) = row else { return };
    let items = match row.art.as_ref().map(|a| a.app.clone()) {
        Some(app) => crate::ctx::card_items(i, &app),
        None => crate::ctx::msg_items(i),
    };
    let mine = row.role == "user";
    spawn(async move {
        // the scroll and the ring land first; then it measures the message
        timer(std::time::Duration::from_millis(120)).await;
        let at = match store().sel_ring.get() {
            Some(r) => r.measure().await,
            None => None,
        };
        match at {
            Some(m) => {
                let (x, y, w, h) = (m.page.x.0, m.page.y.0, m.frame.size.width.0, m.frame.size.height.0);
                let left = if mine { x + w - crate::ctx::CTX_W } else { x };
                crate::ctx::open_near(left, y, y + h, items);
                store().ctx_owner.set(Some((i, "msg".to_owned())));
            }
            None => {
                let win = window();
                crate::ctx::open_at(win.width.0 / 2.0, win.height.0 / 3.0, items);
            }
        }
    });
}

/// ⌘Y: replies to the selected message, as its ↩ does.
fn reply() {
    let st = store();
    if let Some(i) = st.sel_msg.get_untracked() {
        st.reply_to_turn(i);
        crate::composer::focus();
    }
}

/// Sends a menu action down the responder chain (the app delegate's
/// performance monitor, NSApplication's hide, the window's miniaturize)
/// — on the main thread, through `rox/keys`.
fn send_action(action: &'static str) {
    sender().update(|(n, a)| {
        *n += 1;
        *a = action.to_owned();
    });
}

/// The last action `send_action` asked for, numbered.
fn sender() -> Signal<(u64, String)> {
    store().keys_send
}

/// The last print job asked for, numbered: (n, "text" | "image", the text
/// or the image's URL, the job's title).
fn printer() -> Signal<(u64, String, String, String)> {
    store().keys_print
}

/// Opens the system print panel for `payload` — text (monospaced), HTML
/// or an image at a URL — titled `title`.
pub(crate) fn print(kind: &str, payload: String, title: &str) {
    printer().update(|p| {
        p.0 += 1;
        p.1 = kind.to_owned();
        p.2 = payload;
        p.3 = title.to_owned();
    });
}

/// The keymap's user overrides from `GET /keymap`'s body.
pub(crate) fn overrides_of(v: &serde_json::Value) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    if let Some(m) = v.get("keymap").and_then(|m| m.as_object()) {
        for (id, keys) in m {
            if let Some(a) = keys.as_array() {
                out.insert(
                    id.clone(),
                    a.iter().filter_map(|k| k.as_str()).filter(|k| parse(k).is_some()).map(str::to_owned).collect(),
                );
            }
        }
    }
    out
}

/// `text` as a keystroke (Apple conventions: `mod` is Command).
pub(crate) fn parse(text: &str) -> Option<Keystroke> {
    Keystroke::parse_with(text, true).ok()
}

/// The keystrokes `c` answers to now: the user's, else its defaults.
pub(crate) fn keys_of(c: &Command) -> Vec<String> {
    store()
        .keymap
        .with(|m| m.get(c.id).cloned())
        .unwrap_or_else(|| c.keys.iter().map(|k| (*k).to_owned()).collect())
}

/// Sets `id`'s keystrokes (its defaults again when they match) and saves
/// the keymap.
pub(crate) fn set_keys(id: &str, keys: Vec<String>) {
    let Some(c) = COMMANDS.iter().find(|c| c.id == id) else { return };
    let defaults: Vec<String> = c.keys.iter().map(|k| (*k).to_owned()).collect();
    let st = store();
    st.keymap.update(|m| {
        if keys == defaults {
            m.remove(id);
        } else {
            m.insert(id.to_owned(), keys);
        }
    });
    save();
}

/// Every command back to its defaults.
pub(crate) fn reset_all() {
    store().keymap.set(BTreeMap::new());
    save();
}

fn save() {
    let map = store().keymap.get_untracked();
    crate::client::post_bg("/keymap".to_owned(), serde_json::json!({ "keymap": map }));
}

/// The other commands that answer to `key` where `c` does: its own scope,
/// or Global (open with every scope) on either side.
pub(crate) fn conflicts(c: &Command, key: &str) -> Vec<&'static Command> {
    let Some(k) = parse(key) else { return Vec::new() };
    COMMANDS
        .iter()
        .filter(|o| o.id != c.id && (o.scope == c.scope || o.scope == Scope::Global || c.scope == Scope::Global))
        .filter(|o| keys_of(o).iter().any(|ok| parse(ok) == Some(k)))
        .collect()
}

/// The commands that own `k` in the open scopes, innermost first.
fn owners(k: Keystroke, open: &[Scope]) -> Option<&'static Command> {
    open.iter().find_map(|s| {
        COMMANDS
            .iter()
            .find(|c| c.scope == *s && keys_of_untracked(c).iter().any(|ck| parse(ck) == Some(k)))
    })
}

fn keys_of_untracked(c: &Command) -> Vec<String> {
    untracked(|| keys_of(c))
}

/// Mounts the bindings: rebuilt when the open scopes or the keymap
/// change. Put once in the root.
pub(crate) fn bindings() -> impl IntoElement {
    let open = computed(|| ORDER.iter().copied().filter(|s| s.open()).collect::<Vec<Scope>>());
    dynamic(move || {
        let open = open.get();
        let keymap = store().keymap.get();
        let _ = keymap;
        // A command whose key is still its menu item's leaves it to the
        // menu; every other key is bound here.
        let menu_held = menu_plan().0;
        if open.first() == Some(&Scope::Recording) {
            return view().into_element();
        }
        let mut bound: BTreeSet<String> = BTreeSet::new();
        for s in &open {
            for c in COMMANDS.iter().filter(|c| c.scope == *s) {
                for key in keys_of_untracked(c) {
                    let Some(k) = parse(&key) else { continue };
                    let canon = k.to_string();
                    if bound.contains(&canon) || menu_held.contains(&canon) {
                        continue;
                    }
                    // the innermost open scope's command owns the key
                    let Some(owner) = owners(k, &open) else { continue };
                    bound.insert(canon.clone());
                    let options = KeyOptions { when: owner.when, repeat: owner.repeat };
                    let open2 = open.clone();
                    let fire = move |_: &str| {
                        if let Some(c) = owners(k, &open2) {
                            (c.run)(c.arg);
                        }
                    };
                    if k.reserved(nsh_core::platform::Os::MacOs).is_some() {
                        on_key_claiming(&canon, options, fire);
                    } else {
                        on_key_with(&canon, options, fire);
                    }
                }
            }
        }
        view().into_element()
    })
}

/// A letter answers the newest open choice card (A its first option), the
/// way Grok Bot's keycaps work: only while no text field has the caret
/// (`NotEditing`), no modifier is held (the plain key), and the chat is
/// the open scope with no menu or popup over it. Bound only while such a
/// card is there, and only its options' letters, so the rest still type.
pub(crate) fn choice_keys() -> impl IntoElement {
    let options = computed(|| {
        let free = Scope::Chat.open() && !Scope::Menu.open() && !Scope::ComposerPopup.open();
        free.then(crate::cards::choice::keyed)
            .flatten()
            .map(|(_, spec)| spec.fields.first().map_or(0, |f| f.options.len()).min(26))
            .unwrap_or(0)
    });
    dynamic(move || {
        for index in 0..options.get() {
            let Some(letter) = crate::cards::choice::choice_letter(index) else { continue };
            let options = KeyOptions { when: When::NotEditing, repeat: false };
            on_key_with(&letter.to_ascii_lowercase().to_string(), options, move |_| {
                crate::cards::choice::answer_keyed(index)
            });
        }
        view().into_element()
    })
}

/// What the menu bar does with its items' keys: per menu action, whether
/// its item keeps its own key (its command still answers to just that
/// key and no other command claims it); and the keys the menu keeps.
fn menu_plan() -> (BTreeSet<String>, serde_json::Map<String, serde_json::Value>) {
    let claimed: Vec<(&str, Vec<String>)> = COMMANDS
        .iter()
        .map(|c| (c.id, keys_of(c).iter().filter_map(|k| parse(k)).map(|k| k.to_string()).collect()))
        .collect();
    let mut held = BTreeSet::new();
    let mut plan = serde_json::Map::new();
    for c in COMMANDS.iter() {
        let Some(m) = c.menu else { continue };
        let Some(default) = parse(m.key).map(|k| k.to_string()) else { continue };
        let mine = claimed.iter().find(|(id, _)| *id == c.id).map(|(_, k)| k.clone()).unwrap_or_default();
        let others = claimed.iter().any(|(id, keys)| *id != c.id && keys.contains(&default));
        let keep = mine == vec![default.clone()] && !others;
        if keep {
            held.insert(default);
        }
        plan.insert(m.action.to_owned(), serde_json::Value::Bool(keep));
    }
    (held, plan)
}

/// `rox/keys`, a 0×0 element at the root that does the keymap's AppKit
/// work on the main thread: the menu items' key equivalents, the actions
/// commands send, and Settings' key recorder (its keys come back as `key`
/// events).
pub(crate) fn host() -> impl IntoElement {
    native("rox/keys")
        .prop("menu", || serde_json::Value::Object(menu_plan().1))
        .prop("send", || {
            let (n, a) = sender().get();
            serde_json::json!([n, a])
        })
        .prop("record", || serde_json::Value::Bool(store().key_record.get().is_some()))
        .prop("print", || {
            let (n, kind, payload, title) = printer().get();
            serde_json::json!([n, kind, payload, title])
        })
        .on_native_event(|e: &NativeEvent| {
            if &*e.name == "key" {
                if let Some(k) = e.data.as_str() {
                    crate::settings::recorded(k);
                }
            }
        })
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size(px(0.0))
}

#[cfg(target_os = "macos")]
mod imp {
    use std::collections::BTreeMap;
    use std::ptr::NonNull;

    use nsh_core::JsonValue;
    use nsh_platform_macos::{
        ComponentContext, NativeComponent,
        block2::RcBlock,
        keystroke_of,
        objc2::{
            MainThreadMarker, MainThreadOnly,
            rc::Retained,
            runtime::{AnyObject, Sel},
        },
        objc2_app_kit::{NSApplication, NSEvent, NSEventMask, NSEventModifierFlags, NSMenu, NSMenuItem, NSView},
        objc2_foundation::{NSPoint, NSRect, NSSize, NSString},
    };

    pub struct KeysHost {
        view: Retained<NSView>,
        ctx: ComponentContext,
        /// Each touched menu item's own key equivalent and modifiers.
        originals: BTreeMap<String, (String, NSEventModifierFlags)>,
        /// The recorder's key monitor, while Settings records.
        monitor: Option<Retained<AnyObject>>,
        /// The last action sent (its number).
        sent: u64,
        /// The last print job run (its number).
        printed: u64,
    }

    impl KeysHost {
        pub fn new(ctx: ComponentContext) -> KeysHost {
            let mtm = MainThreadMarker::new().expect("a component is made on the main thread");
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
            let view = NSView::initWithFrame(NSView::alloc(mtm), frame);
            KeysHost { view, ctx, originals: BTreeMap::new(), monitor: None, sent: 0, printed: 0 }
        }

        fn item_for(action: &str) -> Option<Retained<NSMenuItem>> {
            fn items(menu: &NSMenu, out: &mut Vec<Retained<NSMenuItem>>) {
                for i in 0..menu.numberOfItems() {
                    if let Some(item) = menu.itemAtIndex(i) {
                        if let Some(sub) = item.submenu() {
                            items(&sub, out);
                        }
                        out.push(item);
                    }
                }
            }
            let mtm = MainThreadMarker::new()?;
            let menu = NSApplication::sharedApplication(mtm).mainMenu()?;
            let mut all = Vec::new();
            items(&menu, &mut all);
            let sel = Sel::register(&std::ffi::CString::new(action).ok()?);
            all.into_iter().find(|it| it.action() == Some(sel))
        }

        /// `{action: keep}`: an item that keeps its key gets it back,
        /// any other shows none (the keymap binds the key).
        fn menu(&mut self, plan: &serde_json::Map<String, JsonValue>) {
            for (action, keep) in plan {
                let Some(item) = Self::item_for(action) else { continue };
                let original = self
                    .originals
                    .entry(action.clone())
                    .or_insert_with(|| (item.keyEquivalent().to_string(), item.keyEquivalentModifierMask()))
                    .clone();
                if keep.as_bool() == Some(true) {
                    item.setKeyEquivalent(&NSString::from_str(&original.0));
                    item.setKeyEquivalentModifierMask(original.1);
                } else {
                    item.setKeyEquivalent(&NSString::from_str(""));
                }
            }
        }

        fn send(action: &str) {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let Ok(name) = std::ffi::CString::new(action) else { return };
            // SAFETY: a menu action taking a sender, sent to the first
            // responder that implements it.
            unsafe {
                NSApplication::sharedApplication(mtm).sendAction_to_from(Sel::register(&name), None, None);
            }
        }

        /// Runs the print panel on the next pass of the run loop (not
        /// inside the engine's prop update).
        fn print(kind: String, payload: String, title: String) {
            use nsh_platform_macos::objc2_foundation::NSRunLoop;
            let block = RcBlock::new(move || print_now(&kind, &payload, &title));
            // SAFETY: the block runs once, on this (the main) thread's loop.
            unsafe { NSRunLoop::currentRunLoop().performBlock(&block) };
        }

        fn record(&mut self, on: bool) {
            if !on {
                if let Some(m) = self.monitor.take() {
                    // SAFETY: a monitor `addLocalMonitorForEventsMatchingMask` returned.
                    unsafe { NSEvent::removeMonitor(&m) };
                }
                return;
            }
            if self.monitor.is_some() {
                return;
            }
            let ctx = self.ctx.clone();
            let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
                // SAFETY: AppKit hands the monitor a live event.
                let ev = unsafe { event.as_ref() };
                match keystroke_of(ev) {
                    Some(k) => {
                        ctx.emit("key", JsonValue::String(k.to_string()));
                        std::ptr::null_mut()
                    }
                    None => event.as_ptr(),
                }
            });
            // SAFETY: the block returns the event or nil, as a key monitor must.
            self.monitor =
                unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block) };
        }
    }

    /// The print panel for text (monospaced, wrapped to the page), HTML
    /// (laid out) or an image (at a URL, fit to the page's width).
    fn print_now(kind: &str, payload: &str, title: &str) {
        use objc2_app_kit::{NSFont, NSImage, NSImageView, NSPrintInfo, NSPrintOperation, NSTextView};
        use nsh_platform_macos::objc2::AllocAnyThread;
        use nsh_platform_macos::objc2_foundation::NSURL;
        let Some(mtm) = MainThreadMarker::new() else { return };
        // HTML prints from the web view showing it, with its CSS (a BIR
        // return's boxes, @page sizes); AppKit's HTML reader keeps text only
        if kind == "html" && print_web_view(mtm, title, None) {
            return;
        }
        // dev hook (`printpdf <path>`): the viewer's page into that PDF
        if let Some(path) = kind.strip_prefix("pdf:") {
            print_web_view(mtm, title, Some(path));
            return;
        }
        let info = NSPrintInfo::sharedPrintInfo();
        let paper = info.paperSize();
        let width = (paper.width - info.leftMargin() - info.rightMargin()).max(100.0);
        let view: Retained<NSView> = if kind == "image" {
            let Some(url) = NSURL::URLWithString(&NSString::from_str(payload)) else { return };
            let Some(image) = NSImage::initWithContentsOfURL(NSImage::alloc(), &url) else { return };
            let size = image.size();
            let w = size.width.min(width).max(1.0);
            let h = if size.width > 0.0 { size.height * w / size.width } else { size.height };
            let iv = NSImageView::imageViewWithImage(&image, mtm);
            iv.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)));
            Retained::into_super(Retained::into_super(iv))
        } else if kind == "html" {
            // laid out by AppKit's HTML reader (its tables and styles),
            // wrapped to the page
            use nsh_platform_macos::objc2::{msg_send, rc::Allocated, runtime::AnyClass};
            use nsh_platform_macos::objc2_foundation::NSData;
            let tv = NSTextView::initWithFrame(
                NSTextView::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, 100.0)),
            );
            let data = NSData::with_bytes(payload.as_bytes());
            let Some(cls) = AnyClass::get(c"NSAttributedString") else { return };
            // SAFETY: `alloc` then NSAttributedString's AppKit
            // `initWithHTML:documentAttributes:` (no attributes asked for);
            // `textStorage` is an NSMutableAttributedString.
            unsafe {
                let obj: Allocated<AnyObject> = msg_send![cls, alloc];
                let html: Option<Retained<AnyObject>> = msg_send![
                    obj,
                    initWithHTML: &*data,
                    documentAttributes: std::ptr::null_mut::<*mut AnyObject>()
                ];
                let Some(html) = html else { return };
                let storage: Option<Retained<AnyObject>> = msg_send![&*tv, textStorage];
                if let Some(storage) = storage {
                    let () = msg_send![&*storage, setAttributedString: &*html];
                }
            }
            tv.setVerticallyResizable(true);
            tv.sizeToFit();
            Retained::into_super(Retained::into_super(tv))
        } else {
            let tv = NSTextView::initWithFrame(
                NSTextView::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, 100.0)),
            );
            tv.setString(&NSString::from_str(payload));
            if let Some(font) = NSFont::userFixedPitchFontOfSize(9.5) {
                tv.setFont(Some(&font));
            }
            tv.setVerticallyResizable(true);
            tv.sizeToFit();
            Retained::into_super(Retained::into_super(tv))
        };
        let op = NSPrintOperation::printOperationWithView(&view);
        op.setJobTitle(Some(&NSString::from_str(title)));
        op.runOperation();
    }

    /// The viewer's page printed by its own WKWebView (the largest one
    /// showing in the key window), sheet on that window. False when none
    /// shows.
    fn print_web_view(mtm: MainThreadMarker, title: &str, save_to: Option<&str>) -> bool {
        use nsh_platform_macos::objc2::{Message, msg_send, runtime::{AnyClass, AnyObject}};
        use nsh_platform_macos::objc2_foundation::NSCopying;
        use objc2_app_kit::{NSApplication, NSPrintInfo, NSPrintingPaginationMode};
        // by kind, not name: KVO renames the class (`NSKVONotifying_WKWebView`)
        fn find(view: &NSView, web: &AnyClass, best: &mut Option<(f64, Retained<NSView>)>) {
            if view.isHiddenOrHasHiddenAncestor() {
                return;
            }
            // SAFETY: NSObject's `isKindOfClass:`.
            let is_web: bool = unsafe { msg_send![view, isKindOfClass: web] };
            if is_web {
                let b = view.visibleRect();
                let area = b.size.width * b.size.height;
                if area > best.as_ref().map_or(0.0, |(a, _)| *a) {
                    *best = Some((area, view.retain()));
                }
                return;
            }
            for sub in view.subviews().iter() {
                find(&sub, web, best);
            }
        }
        let Some(web_class) = AnyClass::get(c"WKWebView") else { return false };
        let app = NSApplication::sharedApplication(mtm);
        // the key window; when Rox isn't frontmost, its first visible one
        let window = app.keyWindow().or_else(|| app.mainWindow()).or_else(|| {
            app.windows().iter().find(|w| w.isVisible())
        });
        let Some(window) = window else { return false };
        let Some(content) = window.contentView() else { return false };
        let mut best = None;
        find(&content, web_class, &mut best);
        let Some((_, web)) = best else { return false };
        // a copy: the shared info stays as the app's other prints left it
        let info: Retained<NSPrintInfo> = NSPrintInfo::sharedPrintInfo().copy();
        info.setHorizontalPagination(NSPrintingPaginationMode::Fit);
        info.setVerticallyCentered(false);
        for set in [NSPrintInfo::setTopMargin, NSPrintInfo::setBottomMargin, NSPrintInfo::setLeftMargin, NSPrintInfo::setRightMargin] {
            set(&info, 18.0);
        }
        // SAFETY: WKWebView's `printOperationWithPrintInfo:` (macOS 11+)
        // returns an NSPrintOperation; its view needs the web view's size or
        // WebKit prints a blank page, and a print run modal for the window
        // (not `runOperation`) is the one WebKit lays out for.
        unsafe {
            // dev hook (`printpdf <path>`): straight to a PDF, no panel
            if let Some(path) = save_to {
                use nsh_platform_macos::objc2_foundation::NSURL;
                let () = msg_send![&*info, setJobDisposition: &*NSString::from_str("NSPrintSaveJob")];
                let dict: Retained<AnyObject> = msg_send![&*info, dictionary];
                let url = NSURL::fileURLWithPath(&NSString::from_str(path));
                let () = msg_send![&*dict, setObject: &*url, forKey: &*NSString::from_str("NSPrintJobSavingURL")];
            }
            let op: Option<Retained<AnyObject>> = msg_send![&*web, printOperationWithPrintInfo: &*info];
            let Some(op) = op else { return false };
            let pv: Option<Retained<NSView>> = msg_send![&*op, view];
            if let Some(pv) = pv {
                pv.setFrame(web.bounds());
            }
            let () = msg_send![&*op, setJobTitle: &*NSString::from_str(title)];
            let () = msg_send![&*op, setShowsPrintPanel: save_to.is_none()];
            let () = msg_send![&*op, setShowsProgressPanel: save_to.is_none()];
            let () = msg_send![
                &*op,
                runOperationModalForWindow: &*window,
                delegate: std::ptr::null::<AnyObject>(),
                didRunSelector: std::ptr::null::<std::ffi::c_void>(),
                contextInfo: std::ptr::null_mut::<std::ffi::c_void>()
            ];
        }
        true
    }

    impl Drop for KeysHost {
        fn drop(&mut self) {
            self.record(false);
        }
    }

    impl NativeComponent for KeysHost {
        fn view(&self) -> Retained<NSView> {
            self.view.clone()
        }

        fn set_prop(&mut self, name: &str, value: Option<&JsonValue>) {
            match name {
                "menu" => {
                    if let Some(plan) = value.and_then(|v| v.as_object()) {
                        self.menu(plan);
                    }
                }
                "send" => {
                    let a = value.and_then(|v| v.as_array());
                    let n = a.and_then(|a| a.first()?.as_u64()).unwrap_or(0);
                    if n > self.sent {
                        self.sent = n;
                        if let Some(action) = a.and_then(|a| a.get(1)?.as_str()) {
                            Self::send(action);
                        }
                    }
                }
                "record" => self.record(value.and_then(|v| v.as_bool()) == Some(true)),
                "print" => {
                    let a = value.and_then(|v| v.as_array());
                    let n = a.and_then(|a| a.first()?.as_u64()).unwrap_or(0);
                    let s = |i: usize| a.and_then(|a| a.get(i)?.as_str()).unwrap_or_default().to_owned();
                    if n > self.printed {
                        self.printed = n;
                        Self::print(s(1), s(2), s(3));
                    }
                }
                _ => {}
            }
        }
    }
}

/// Registers `rox/keys` (call before `run_app`, as `register_zoom`).
#[cfg(target_os = "macos")]
pub fn register_keys(components: &nsh_platform_macos::Components) {
    components.register(
        "rox/keys",
        std::rc::Rc::new(|ctx| Box::new(imp::KeysHost::new(ctx)) as Box<dyn nsh_platform_macos::NativeComponent>),
    );
}
