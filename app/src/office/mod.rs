//! Office files in native views: Excel first (`xlsx/`), Word and
//! PowerPoint later. Each format is a [`session::Document`] on a worker
//! thread, drawn from BetterOffice's display lists ([`draw`]).
//!
//! The viewer routes a file here by its extension while the
//! `office.native` setting is on (the default; kept in `/state/rox-office`):
//! every other format, and these with the setting off, keep the web view.
//! A file opens natively only where Rox can read and watch it: a file on
//! this Mac, or a reply's file in the broker's files folder (whose saves
//! go through `/file/write`, so they become versions as the web editor's
//! did).

pub(crate) mod draw;
pub(crate) mod session;
pub(crate) mod xlsx;

use std::cell::RefCell;
use std::path::PathBuf;

use nsh_core::prelude::*;

use crate::live::store;
use crate::store::Attach;
use session::{Event, Session, Sink, State};
use xlsx::doc::{Book, Out};

/// The open native document's UI state, kept in the store (one viewer).
pub(crate) struct Ui {
    /// The `office.native` setting.
    pub native: Signal<bool>,
    /// The viewer path of the file open natively ("" when none).
    pub key: Signal<String>,
    pub state: Signal<State>,
    pub sheet: xlsx::SheetUi,
    /// Closing asked while the document holds edits.
    pub ask_close: Signal<bool>,
}

impl Ui {
    pub(crate) fn new() -> Ui {
        Ui { native: signal(true), key: signal(String::new()), state: signal(State::default()), sheet: xlsx::SheetUi::new(), ask_close: signal(false) }
    }
}

fn ui() -> &'static Ui {
    &store().office
}

struct Open {
    key: String,
    session: Session<Book>,
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
    matches!(ext.as_str(), "xlsx" | "xlsm" | "xls" | "csv" | "tsv")
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
    if !u.native.get_untracked() || !native_ext(&f.path) {
        return false;
    }
    let Some((path, sink, files)) = file_of(f) else { return false };
    let generation = GENERATION.with(|g| {
        g.set(g.get() + 1);
        g.get()
    });
    batch(|| {
        u.key.set(f.path.clone());
        u.state.set(State { path: path.clone(), ..State::default() });
        u.ask_close.set(false);
        u.sheet.reset();
    });
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let session = Session::<Book>::open(path, sink, move |e| {
        let _ = tx.send(e);
    });
    crate::store::spawn_detached(async move {
        while let Some(e) = rx.recv().await {
            if OPEN.with_borrow(|o| o.as_ref().map(|o| o.generation)) != Some(generation) {
                return;
            }
            on_event(e);
        }
    });
    OPEN.with_borrow_mut(|o| *o = Some(Open { key: f.path.clone(), session, files, generation }));
    true
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

/// Runs `f` with the open session.
pub(crate) fn with_session(f: impl FnOnce(&Session<Book>)) {
    OPEN.with_borrow(|o| {
        if let Some(o) = o {
            f(&o.session)
        }
    });
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
            with_session(Session::save);
        }
    }
}

thread_local! {
    static CLOSE_AFTER_SAVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// ⌘S on a native document.
pub(crate) fn save() {
    with_session(Session::save);
}

fn on_event(e: Event<Out>) {
    let u = ui();
    match e {
        Event::State(s) => u.state.set(s),
        Event::Out(o) => xlsx::on_out(&u.sheet, o),
        Event::Reloaded => u.sheet.reloaded(),
        Event::Saved(path) => {
            let shown = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if xlsx::ext(&path) == "xlsx" && !native_ext_same(&u.key.get_untracked(), &path) {
                crate::toast::show(&format!("Saved as {shown}"));
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
        with_session(|s| s.uploaded(version, false, "The file is outside the broker's files".to_owned()));
        return;
    };
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    crate::store::spawn_detached(async move {
        let answer = crate::client::post("/file/write", serde_json::json!({ "path": rel, "data": data })).await;
        let ok = answer.as_ref().and_then(|a| a.get("ok")).and_then(|v| v.as_bool()).unwrap_or(false);
        let error = answer.as_ref().and_then(|a| a.get("error")).and_then(|v| v.as_str()).unwrap_or("The broker didn't answer").to_owned();
        with_session(|s| s.uploaded(version, ok, if ok { String::new() } else { format!("Couldn't save: {error}") }));
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

/// The ⌘K switch: Office files in the native views or the web editor.
pub(crate) fn toggle_native() {
    let u = ui();
    let on = !u.native.get_untracked();
    u.native.set(on);
    crate::client::post_bg("/state/rox-office".to_owned(), serde_json::json!({ "native": on }));
    // the open file switches views
    if let Some(f) = store().viewing.get_untracked()
        && (native_ext(&f.path) && (on || showing()))
    {
        if may_close() {
            store().open_file(&f);
        }
    }
}
