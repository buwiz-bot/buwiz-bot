//! What the office views need of the app that hosts them, given once at
//! startup ([`install`]).
//!
//! The app owns the open document's lifetime: it routes a file to its
//! view, opens and closes the document's [`Session`], answers the close
//! prompt and uploads a save. The views reach that through [`Host`], with
//! the file viewer around them, toasts, the context menu, printing and the
//! app theme's tokens beyond the kit's own palette ([`crate::ui::palette`]).
//! Each format's view state lives in the app's store, and the session is
//! the app's: a format reaches both through its [`Format`].

use std::sync::OnceLock;

use nsh_core::prelude::*;

use crate::session::{Document, Session, State};

/// The app, as the office views see it. Reads marked *tracked* subscribe
/// the computation they run in, as reading a signal does.
pub trait Host: Send + Sync + 'static {
    /// Whether the viewer shows a native document (tracked).
    fn showing(&self) -> bool;
    /// The open document's state on its worker.
    fn state(&self) -> Signal<State>;
    /// The open document is a Word document (else a spreadsheet).
    fn is_word(&self) -> Signal<bool>;
    /// Closing asked while the document holds edits: the prompt shows.
    fn ask_close(&self) -> Signal<bool>;
    /// Whether the viewer may close now: a document with unsaved edits
    /// asks first (the prompt) and says no.
    fn may_close(&self) -> bool;
    /// Drops the open document (its worker stops).
    fn close(&self);
    /// The prompt's answer: `Some(true)` saves first, `Some(false)` drops
    /// the edits, `None` stays.
    fn answer_close(&self, save: Option<bool>);
    /// ⌘S on the open document.
    fn save(&self);
    /// Takes the file as it changed outside the app (dropping the edits).
    fn reload(&self);
    /// Keeps the edits over the outside change.
    fn keep_mine(&self);

    /// The name of the file the viewer shows (tracked).
    fn viewing_name(&self) -> Option<String>;
    /// Closes the viewer.
    fn close_viewer(&self);
    /// The viewer's Save.
    fn viewer_save(&self);
    /// The viewer's Print.
    fn viewer_print(&self);

    /// A link out of a document, opened as the app opens links.
    fn open_link(&self, url: &str);
    /// `payload` (of `kind`, e.g. `html`) to the print panel as `title`.
    fn print(&self, kind: &str, payload: String, title: &str);
    /// A toast.
    fn toast(&self, text: String);
    /// Whether the app's context menu is open (tracked).
    fn menu_open(&self) -> bool;
    /// The app's context menu at `(x, y)`: a row's verb comes back to the
    /// view through the app's menu actions.
    fn context_menu(&self, x: f32, y: f32, rows: Vec<MenuRow>);
    /// The app theme's tokens the views draw their chrome with.
    fn theme(&self) -> Theme;
}

/// A row of the app's context menu.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuRow {
    /// A labelled row: its icon's name and the verb the app runs for it
    /// (`act`, given `arg`).
    Row { label: String, icon: String, act: String, arg: String },
    Separator,
}

impl MenuRow {
    pub fn row(label: &str, icon: &str, act: &str, arg: String) -> MenuRow {
        MenuRow::Row { label: label.to_owned(), icon: icon.to_owned(), act: act.to_owned(), arg }
    }

    pub fn separator() -> MenuRow {
        MenuRow::Separator
    }
}

/// The app theme's tokens beyond the kit's palette: the spreadsheet's
/// chrome and the prompts take the app's look.
#[derive(Clone, Copy)]
pub struct Theme {
    pub pane: AdaptiveColor,
    pub card: AdaptiveColor,
    pub editor: AdaptiveColor,
    pub text: AdaptiveColor,
    pub muted: AdaptiveColor,
    pub faint: AdaptiveColor,
    pub border_s: AdaptiveColor,
    pub hover: AdaptiveColor,
    pub active: AdaptiveColor,
    pub subtle: AdaptiveColor,
    pub on_fill: AdaptiveColor,
    pub accent: AdaptiveColor,
    pub accent_tint: AdaptiveColor,
}

static HOST: OnceLock<Box<dyn Host>> = OnceLock::new();

/// The app, from here on (once: a second call is ignored).
pub fn install(host: impl Host) {
    let _ = HOST.set(Box::new(host));
}

/// The app that hosts the views.
pub fn host() -> &'static dyn Host {
    HOST.get().map(|h| &**h).expect("the office host is installed at startup (buwiz_office_kit::host::install)")
}

/// The app theme's tokens.
pub fn theme() -> Theme {
    host().theme()
}

/// Runs a closure with the open document's session, if it is a `D`.
pub type WithSession<D> = fn(&mut dyn FnMut(&Session<D>));

/// What a format takes from the app: where its view state is kept, and the
/// open document's session while it is one of this format.
pub struct Hooks<U: 'static, D: Document> {
    pub ui: fn() -> &'static U,
    pub with_session: WithSession<D>,
}

/// A format's [`Hooks`], given once at startup.
pub struct Format<U: 'static, D: Document> {
    hooks: OnceLock<Hooks<U, D>>,
}

impl<U: 'static, D: Document> Format<U, D> {
    pub const fn new() -> Self {
        Format { hooks: OnceLock::new() }
    }

    /// The app's hooks, from here on (once: a second call is ignored).
    pub fn install(&self, hooks: Hooks<U, D>) {
        let _ = self.hooks.set(hooks);
    }

    fn hooks(&self) -> &Hooks<U, D> {
        self.hooks.get().expect("the format's hooks are installed at startup (Format::install)")
    }

    /// The format's view state.
    pub fn ui(&self) -> &'static U {
        (self.hooks().ui)()
    }

    /// Runs `f` with the open document's session, if it is of this format.
    pub fn with_session(&self, f: impl FnOnce(&Session<D>)) {
        let mut f = Some(f);
        (self.hooks().with_session)(&mut |s| {
            if let Some(f) = f.take() {
                f(s)
            }
        });
    }
}

impl<U: 'static, D: Document> Default for Format<U, D> {
    fn default() -> Self {
        Self::new()
    }
}
