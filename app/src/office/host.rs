//! The app as the office views' host (`buwiz_office_kit::host`): the open
//! document's lifetime in [`super`], the viewer in `files.rs`, toasts, the
//! context menu, printing and the theme.

use buwiz_office_kit::host::{self, Hooks, Host, MenuRow, Theme};
use buwiz_office_kit::session::State;
use nsh_core::prelude::*;

use crate::live::store;
use crate::store::CtxItem;
use crate::theme;

struct App;

impl Host for App {
    fn showing(&self) -> bool {
        super::showing()
    }
    fn state(&self) -> Signal<State> {
        store().office.state
    }
    fn is_word(&self) -> Signal<bool> {
        store().office.is_word
    }
    fn ask_close(&self) -> Signal<bool> {
        store().office.ask_close
    }
    fn may_close(&self) -> bool {
        super::may_close()
    }
    fn close(&self) {
        super::close()
    }
    fn answer_close(&self, save: Option<bool>) {
        super::answer_close(save)
    }
    fn save(&self) {
        super::save()
    }
    fn reload(&self) {
        super::reload()
    }
    fn keep_mine(&self) {
        super::keep_mine()
    }

    fn viewing_name(&self) -> Option<String> {
        store().viewing.get().map(|f| f.name)
    }
    fn close_viewer(&self) {
        store().viewing.set(None)
    }
    fn viewer_save(&self) {
        crate::files::viewer_save()
    }
    fn viewer_print(&self) {
        crate::files::viewer_print()
    }

    fn open_link(&self, url: &str) {
        store().open_link(url)
    }
    fn print(&self, kind: &str, payload: String, title: &str) {
        crate::keys::print(kind, payload, title)
    }
    fn toast(&self, text: String) {
        crate::toast::show(text)
    }
    fn menu_open(&self) -> bool {
        store().ctx.get().is_some()
    }
    fn context_menu(&self, x: f32, y: f32, rows: Vec<MenuRow>) {
        let items = rows
            .into_iter()
            .map(|r| match r {
                MenuRow::Row { label, icon, act, arg } => CtxItem::row(&label, &icon, &act, arg),
                MenuRow::Separator => CtxItem::separator(),
            })
            .collect();
        crate::ctx::open_at(x, y, items)
    }
    fn theme(&self) -> Theme {
        Theme {
            pane: theme::PANE,
            card: theme::CARD,
            editor: theme::EDITOR,
            text: theme::TEXT,
            muted: theme::MUTED,
            faint: theme::FAINT,
            border_s: theme::BORDER_S,
            hover: theme::HOVER,
            active: theme::ACTIVE,
            subtle: theme::SUBTLE,
            on_fill: theme::ON_FILL,
            accent: theme::ACCENT,
            accent_tint: theme::ACCENT_TINT,
        }
    }
}

/// The host and each format's hooks, once (the store's office state is
/// made with it).
pub(super) fn install() {
    host::install(App);
    buwiz_spreadsheet::FORMAT.install(Hooks { ui: || &store().office.sheet, with_session: |f| super::with_session(|s| f(s)) });
    buwiz_word::FORMAT.install(Hooks { ui: || &store().office.word, with_session: |f| super::with_word(|s| f(s)) });
}
