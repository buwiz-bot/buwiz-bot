//! Office files in native views: Excel (`crates/spreadsheet`) and Word
//! (`crates/word`), PowerPoint later. Each format is a
//! [`session::Document`] on a worker thread, drawn from BetterOffice's
//! display lists ([`draw`], Word's `draw.rs`).
//!
//! The viewer routes a file here by its extension while the
//! `office.native` setting is on (the default; kept in `/state/rox-office`):
//! every other format, and these with the setting off, keep the web view.
//! A file opens natively only where Rox can read and watch it: a file on
//! this Mac, or a reply's file in the broker's files folder (whose saves
//! go through `/file/write`, so they become versions as the web editor's
//! did).

// The views are library crates (`crates/`): this module routes a file to
// them, owns the open document's lifetime and hosts them (`host`).
#[cfg(test)]
pub(crate) use buwiz_office_kit::draw;
pub(crate) use buwiz_office_kit::{fonts, session, ui};
pub(crate) use buwiz_spreadsheet as xlsx;
pub(crate) use buwiz_word as docx;
mod host;
#[cfg(test)]
mod ui_tests;

use std::cell::RefCell;
use std::path::PathBuf;

use nsh_core::prelude::*;

use crate::live::store;
use crate::store::Attach;
use docx::doc::Word;
use session::{Document, Event, Session, Sink, State};
use xlsx::doc::Book;

/// The open native document's UI state, kept in the store (one viewer).
pub(crate) struct Ui {
    /// The `office.native` setting.
    pub native: Signal<bool>,
    /// The viewer path of the file open natively ("" when none).
    pub key: Signal<String>,
    pub state: Signal<State>,
    pub sheet: xlsx::SheetUi,
    pub word: docx::WordUi,
    /// The open document is a Word document (else a spreadsheet).
    pub is_word: Signal<bool>,
    /// Closing asked while the document holds edits.
    pub ask_close: Signal<bool>,
    /// The viewer's switch: one file in the view the setting doesn't pick
    /// (its viewer path, and whether it shows natively), until another
    /// file opens.
    pub forced: Signal<Option<(String, bool)>>,
}

impl Ui {
    pub(crate) fn new() -> Ui {
        // the views reach the app (and this state) from here on
        host::install();
        Ui {
            native: signal(true),
            key: signal(String::new()),
            state: signal(State::default()),
            sheet: xlsx::SheetUi::new(),
            word: docx::WordUi::new(),
            is_word: signal(false),
            ask_close: signal(false),
            forced: signal(None),
        }
    }
}

fn ui() -> &'static Ui {
    &store().office
}

/// The open document's session, by format.
enum Doc {
    Sheet(Session<Book>),
    Word(Session<Word>),
}

struct Open {
    key: String,
    session: Doc,
    /// The broker's files folder, for a reply's file (uploads name their
    /// path in it).
    files: Option<PathBuf>,
    /// Bumped by every open: events of a closed one are dropped.
    generation: u64,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The formats with a native view.
pub(crate) fn native_ext(name: &str) -> bool {
    let ext = xlsx::ext(std::path::Path::new(name));
    matches!(ext.as_str(), "xlsx" | "xlsm" | "xls" | "csv" | "tsv") || is_word(name)
}

/// The formats the Word view opens.
pub(crate) fn is_word(name: &str) -> bool {
    matches!(xlsx::ext(std::path::Path::new(name)).as_str(), "docx" | "doc")
}

/// The broker's reply files: `generated/files` in its data folder, which
/// holds the host token too (a test broker's comes from
/// `ROX_HOST_TOKEN_FILE`).
fn files_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let token = crate::client::host_token_path(std::env::var("ROX_HOST_TOKEN_FILE").ok().as_deref(), &home);
    Some(token.parent()?.join("generated/files"))
}

/// The file behind a viewer path, if Rox can read it here.
fn file_of(f: &Attach) -> Option<(PathBuf, Sink, Option<PathBuf>)> {
    if f.path.starts_with('/') {
        return Some((PathBuf::from(&f.path), Sink::File, None));
    }
    if f.path.is_empty() || f.path.starts_with("app:") || f.path.split('/').any(|s| s == ".." || s.is_empty()) {
        return None;
    }
    let dir = files_dir()?;
    let p = dir.join(&f.path);
    p.is_file().then_some((p, Sink::Broker, Some(dir)))
}

/// Opens `f` natively if it has a native view: `false` leaves it to the
/// classic viewer.
pub(crate) fn open(f: &Attach) -> bool {
    close();
    let u = ui();
    // the switch holds for its own file: another one takes the setting
    if u.forced.with_untracked(|o| o.as_ref().is_some_and(|(p, _)| *p != f.path)) {
        u.forced.set(None);
    }
    if !natively(&f.path) || !native_ext(&f.path) {
        return false;
    }
    let Some((path, sink, files)) = file_of(f) else { return false };
    let generation = GENERATION.with(|g| {
        g.set(g.get() + 1);
        g.get()
    });
    let word = is_word(&f.path);
    batch(|| {
        u.key.set(f.path.clone());
        u.state.set(State { path: path.clone(), ..State::default() });
        u.ask_close.set(false);
        u.is_word.set(word);
        u.sheet.reset();
        u.word.reset();
    });
    let session = if word { Doc::Word(start(path, sink, generation, |o| docx::on_out(&ui().word, o))) } else { Doc::Sheet(start(path, sink, generation, |o| xlsx::on_out(&ui().sheet, o))) };
    OPEN.with_borrow_mut(|o| *o = Some(Open { key: f.path.clone(), session, files, generation }));
    true
}

/// Whether `path` opens natively: the viewer's switch for it, else the
/// setting.
fn natively(path: &str) -> bool {
    let u = ui();
    u.forced.with_untracked(|o| o.as_ref().filter(|(p, _)| p == path).map(|(_, on)| *on)).unwrap_or_else(|| u.native.get_untracked())
}

/// A session on `path`, its answers going to `out` while it is the open one.
fn start<D: Document>(path: PathBuf, sink: Sink, generation: u64, out: fn(D::Out)) -> Session<D> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let session = Session::<D>::open(path, sink, move |e| {
        let _ = tx.send(e);
    });
    crate::store::spawn_detached(async move {
        while let Some(e) = rx.recv().await {
            if OPEN.with_borrow(|o| o.as_ref().map(|o| o.generation)) != Some(generation) {
                return;
            }
            on_event(e, out);
        }
    });
    session
}

/// Drops the open document (its worker stops).
pub(crate) fn close() {
    OPEN.with_borrow_mut(|o| *o = None);
    let u = ui();
    if !u.key.get_untracked().is_empty() {
        batch(|| {
            u.key.set(String::new());
            u.ask_close.set(false);
        });
    }
}

/// Runs `f` with the open spreadsheet's session.
pub(crate) fn with_session(f: impl FnOnce(&Session<Book>)) {
    OPEN.with_borrow(|o| {
        if let Some(Open { session: Doc::Sheet(s), .. }) = o {
            f(s)
        }
    });
}

/// Runs `f` with the open Word document's session.
pub(crate) fn with_word(f: impl FnOnce(&Session<Word>)) {
    OPEN.with_borrow(|o| {
        if let Some(Open { session: Doc::Word(s), .. }) = o {
            f(s)
        }
    });
}

/// Runs the session's own command (save, reload, keep mine, an upload's
/// answer), whatever its format.
fn control(f: impl Fn(&dyn Control)) {
    OPEN.with_borrow(|o| match o.as_ref().map(|o| &o.session) {
        Some(Doc::Sheet(s)) => f(s),
        Some(Doc::Word(s)) => f(s),
        None => {}
    });
}

/// What every session takes, whatever its document.
trait Control {
    fn save(&self);
    fn reload(&self);
    fn keep_mine(&self);
    fn uploaded(&self, version: u64, ok: bool, error: String);
}

impl<D: Document> Control for Session<D> {
    fn save(&self) {
        Session::save(self)
    }
    fn reload(&self) {
        Session::reload(self)
    }
    fn keep_mine(&self) {
        Session::keep_mine(self)
    }
    fn uploaded(&self, version: u64, ok: bool, error: String) {
        Session::uploaded(self, version, ok, error)
    }
}

/// Takes the file as it changed outside Rox (dropping the edits).
pub(crate) fn reload() {
    control(|s| s.reload());
}

/// Keeps the edits over the outside change.
pub(crate) fn keep_mine() {
    control(|s| s.keep_mine());
}

/// Whether the viewer shows a native document.
pub(crate) fn showing() -> bool {
    !ui().key.get().is_empty()
}

/// Whether the viewer may close now: a document with unsaved edits asks
/// first (Save, Don't Save, Cancel) and says no.
pub(crate) fn may_close() -> bool {
    let u = ui();
    if u.key.get_untracked().is_empty() || !u.state.get_untracked().dirty {
        return true;
    }
    u.ask_close.set(true);
    false
}

/// The answer to [`may_close`]'s question: `Some(true)` saves first,
/// `Some(false)` drops the edits, `None` stays.
pub(crate) fn answer_close(save: Option<bool>) {
    let u = ui();
    u.ask_close.set(false);
    match save {
        None => {}
        Some(false) => {
            close();
            store().viewing.set(None);
        }
        Some(true) => {
            CLOSE_AFTER_SAVE.with(|c| c.set(true));
            save_asking();
        }
    }
}

thread_local! {
    static CLOSE_AFTER_SAVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// ⌘S in the viewer: a legacy `.doc` on this Mac asks where its `.docx`
/// goes (once); everything else saves in place.
pub(crate) fn save_asking() {
    let u = ui();
    let st = u.state.get_untracked();
    let local = OPEN.with_borrow(|o| o.as_ref().is_some_and(|o| o.files.is_none()));
    if u.is_word.get_untracked() && local && xlsx::ext(&st.path) == "doc" {
        return docx::view::save_legacy(st.path);
    }
    save();
}

/// ⌘S on a native document.
pub(crate) fn save() {
    control(|s| s.save());
}

fn on_event<O>(e: Event<O>, out: fn(O)) {
    let u = ui();
    match e {
        Event::State(s) => u.state.set(s),
        Event::Out(o) => out(o),
        Event::Reloaded => {
            u.sheet.reloaded();
            u.word.reloaded();
        }
        Event::Saved(path) => {
            let shown = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !native_ext_same(&u.key.get_untracked(), &path) {
                crate::toast::show(format!("Saved as {shown}"));
            }
            let key = u.key.get_untracked();
            if !key.starts_with('/') {
                crate::files::load_file_versions(&key);
            }
            if CLOSE_AFTER_SAVE.with(|c| c.replace(false)) {
                close();
                store().viewing.set(None);
            }
        }
        Event::Upload { bytes, path, version } => upload(bytes, path, version),
    }
}

/// Whether a save to `path` kept the viewer path's format.
fn native_ext_same(key: &str, path: &std::path::Path) -> bool {
    xlsx::ext(std::path::Path::new(key)) == xlsx::ext(path)
}

/// A reply file's save: the broker writes it and keeps the version.
fn upload(bytes: Vec<u8>, path: PathBuf, version: u64) {
    use base64::Engine as _;
    let rel = OPEN.with_borrow(|o| o.as_ref().and_then(|o| o.files.clone())).and_then(|dir| path.strip_prefix(&dir).ok().map(|p| p.to_string_lossy().into_owned()));
    let Some(rel) = rel else {
        control(|s| s.uploaded(version, false, "The file is outside the broker's files".to_owned()));
        return;
    };
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    crate::store::spawn_detached(async move {
        let answer = crate::client::post("/file/write", serde_json::json!({ "path": rel, "data": data })).await;
        let ok = answer.as_ref().and_then(|a| a.get("ok")).and_then(|v| v.as_bool()).unwrap_or(false);
        let error = answer.as_ref().and_then(|a| a.get("error")).and_then(|v| v.as_str()).unwrap_or("The broker didn't answer").to_owned();
        control(|s| s.uploaded(version, ok, if ok { String::new() } else { format!("Couldn't save: {error}") }));
    });
}

/// The setting, from `/state/rox-office`, and the viewer's lifetime: the
/// document closes when the viewer moves off it (another file, another
/// chat).
pub(crate) fn track() {
    crate::store::spawn_detached(async move {
        if let Some(v) = crate::client::get("/state/rox-office").await
            && let Some(native) = v.get("state").and_then(|s| s.get("native")).and_then(|n| n.as_bool())
        {
            ui().native.set(native);
        }
    });
    effect(|| {
        let viewing = store().viewing.get().map(|f| f.path);
        untracked(|| {
            let open = OPEN.with_borrow(|o| o.as_ref().map(|o| o.key.clone()));
            if open.is_some() && open != viewing {
                close();
            }
        });
    });
}

/// The viewer's "View in Web Editor" / "View Natively": the open Word or
/// Excel file in the other view, this file only (the setting stays). The
/// native document closes first, asking about its edits.
pub(crate) fn switch_view() {
    let Some(f) = store().viewing.get_untracked() else { return };
    if !native_ext(&f.path) || !may_close() {
        return;
    }
    let native = !ui().key.get_untracked().is_empty();
    ui().forced.set(Some((f.path.clone(), !native)));
    store().open_file(&f);
}

/// The switch's label for the open file, if it has one: the web editor
/// renders on the server, so a file on this Mac has none.
pub(crate) fn switch_label(path: &str) -> Option<&'static str> {
    if !native_ext(path) || path.starts_with('/') {
        return None;
    }
    Some(if ui().key.get_untracked().is_empty() { "View Natively" } else { "View in Web Editor" })
}
