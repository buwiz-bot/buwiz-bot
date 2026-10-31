//! One open Office document on a worker thread of its own: the UI sends
//! commands (an edit, undo, a viewport to draw, save) and hears back
//! versioned states and the document's answers, so a slow recalculation
//! or a large save never holds a frame. Shared by the Excel, Word and
//! PowerPoint views; each brings its [`Document`].
//!
//! The worker owns the file too: a save writes a temp file beside it and
//! renames it over (a crash mid-save leaves the old file whole), and the
//! file is watched — a change from outside Rox (the AI's office tools, a
//! sync) reloads it silently while the document is clean and asks while
//! it holds edits.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, SystemTime};

/// What an open document does on its worker. It is made and kept there
/// (never sent across threads: Word's engine is single-threaded).
pub trait Document: Sized + 'static {
    /// The document's own commands (edits, reads, a viewport to draw).
    type Cmd: Send + 'static;
    /// What it answers them with.
    type Out: Send + 'static;

    /// The document in `bytes`, read from `path` (its extension says the
    /// format).
    fn open(bytes: Vec<u8>, path: &Path) -> Result<Self, String>;
    /// Runs `cmd`: what it answers, and whether the document changed (a
    /// new version, unsaved).
    fn run(&mut self, cmd: Self::Cmd) -> (Vec<Self::Out>, bool);
    /// Work the document finishes in the background (a full engine built
    /// after a quick first paint): answers, and whether it changed.
    fn tick(&mut self) -> (Vec<Self::Out>, bool) {
        (Vec::new(), false)
    }
    /// Whether [`Self::tick`] has work coming (the worker polls it often
    /// while it does).
    fn busy(&self) -> bool {
        false
    }
    /// The bytes to save and the file they go to: `path` itself, or
    /// another when the format can't be written back (a legacy `.xls`
    /// saves as `.xlsx`).
    fn save(&mut self, path: &Path) -> Result<(Vec<u8>, PathBuf), String>;
}

/// Where a save goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Sink {
    /// Written in place, atomically.
    File,
    /// A reply's file, which the broker keeps versions of: the UI uploads
    /// the bytes (`/file/write`) and reports back ([`Session::uploaded`]).
    Broker,
}

/// The document's state the UI shows.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct State {
    /// Bumped by every change, a reload included.
    pub version: u64,
    /// It holds edits not saved.
    pub dirty: bool,
    pub saving: bool,
    /// The file changed outside Rox while it held edits: the UI asks.
    pub conflict: bool,
    /// The file it saves to (a `.xls` becomes its `.xlsx` once saved).
    pub path: PathBuf,
    /// The last error (a failed open, save or reload).
    pub error: String,
}

/// What the worker tells the UI.
pub enum Event<O> {
    State(State),
    Out(O),
    /// The bytes to upload for a [`Sink::Broker`] save of `version`, as
    /// the file at `path`.
    Upload { bytes: Vec<u8>, path: PathBuf, version: u64 },
    /// It reloaded from disk (the UI drops what it cached of the old one).
    Reloaded,
    /// It saved.
    Saved(PathBuf),
}

enum Msg<C> {
    Cmd(C),
    Save,
    Uploaded { version: u64, ok: bool, error: String },
    /// The outside change: take it (dropping the edits) or keep ours.
    Reload,
    KeepMine,
    Close,
}

/// The UI's handle: commands go in, events come out of the receiver
/// [`Session::open`] returns.
pub struct Session<D: Document> {
    tx: Sender<Msg<D::Cmd>>,
}

impl<D: Document> Drop for Session<D> {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Close);
    }
}

/// How often the worker looks at the file.
const WATCH_EVERY: Duration = Duration::from_millis(500);
/// How long the worker sleeps between ticks while the document has
/// background work.
const TICK: Duration = Duration::from_millis(20);

/// A file's identity for the watcher: modified time and size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp(Option<SystemTime>, u64);

fn stamp(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    Some(Stamp(m.modified().ok(), m.len()))
}

impl<D: Document> Session<D> {
    /// Opens `path` on a new worker; `emit` hears its events (from the
    /// worker thread).
    pub fn open(path: PathBuf, sink: Sink, emit: impl Fn(Event<D::Out>) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("rox-office".to_owned())
            .spawn(move || Worker::<D, _>::run(path, sink, rx, emit))
            .expect("spawn the office worker");
        Session { tx }
    }

    pub fn send(&self, cmd: D::Cmd) {
        let _ = self.tx.send(Msg::Cmd(cmd));
    }

    pub fn save(&self) {
        let _ = self.tx.send(Msg::Save);
    }

    /// A [`Sink::Broker`] upload of `version` finished.
    pub fn uploaded(&self, version: u64, ok: bool, error: String) {
        let _ = self.tx.send(Msg::Uploaded { version, ok, error });
    }

    pub fn reload(&self) {
        let _ = self.tx.send(Msg::Reload);
    }

    pub fn keep_mine(&self) {
        let _ = self.tx.send(Msg::KeepMine);
    }
}

struct Worker<D: Document, E> {
    doc: Option<D>,
    state: State,
    sink: Sink,
    /// The file as last read or written: anything else is an outside change.
    known: Option<Stamp>,
    /// The version a save in flight writes (edits after it keep it dirty).
    saving: Option<u64>,
    emit: E,
}

impl<D: Document, E: Fn(Event<D::Out>)> Worker<D, E> {
    fn run(path: PathBuf, sink: Sink, rx: Receiver<Msg<D::Cmd>>, emit: E) {
        // what it draws names the bundled fonts: registered first
        super::fonts::register();
        let mut w: Worker<D, E> = Worker { doc: None, state: State { path: path.clone(), ..State::default() }, sink, known: None, saving: None, emit };
        w.load(false);
        let mut last_watch = std::time::Instant::now();
        loop {
            let busy = w.doc.as_ref().is_some_and(D::busy);
            let wait = if busy { TICK } else { WATCH_EVERY };
            match rx.recv_timeout(wait) {
                Ok(Msg::Close) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(msg) => w.handle(msg),
                Err(RecvTimeoutError::Timeout) => {}
            }
            // commands queued behind this one run first: a burst of
            // keys or scrolls is one state
            while let Ok(msg) = rx.try_recv() {
                if matches!(msg, Msg::Close) {
                    return;
                }
                w.handle(msg);
            }
            if let Some(doc) = w.doc.as_mut() {
                let (outs, changed) = doc.tick();
                w.answer(outs, changed);
            }
            if last_watch.elapsed() >= WATCH_EVERY {
                last_watch = std::time::Instant::now();
                w.watch();
            }
        }
    }

    fn handle(&mut self, msg: Msg<D::Cmd>) {
        match msg {
            Msg::Cmd(cmd) => {
                let Some(doc) = self.doc.as_mut() else { return };
                let (outs, changed) = doc.run(cmd);
                self.answer(outs, changed);
            }
            Msg::Save => self.save(),
            Msg::Uploaded { version, ok, error } => {
                if self.saving != Some(version) {
                    return;
                }
                self.saving = None;
                self.state.saving = false;
                if ok {
                    self.known = stamp(&self.state.path);
                    self.state.dirty = self.state.version != version;
                    self.state.error.clear();
                    (self.emit)(Event::Saved(self.state.path.clone()));
                } else {
                    self.state.error = error;
                }
                self.publish();
            }
            Msg::Reload => {
                self.state.conflict = false;
                self.load(true);
            }
            Msg::KeepMine => {
                self.state.conflict = false;
                self.known = stamp(&self.state.path);
                self.publish();
            }
            Msg::Close => {}
        }
    }

    fn answer(&mut self, outs: Vec<D::Out>, changed: bool) {
        if changed {
            self.state.version += 1;
            self.state.dirty = true;
        }
        for out in outs {
            (self.emit)(Event::Out(out));
        }
        if changed {
            self.publish();
        }
    }

    fn publish(&self) {
        (self.emit)(Event::State(self.state.clone()));
    }

    /// Reads the file into a new document; `again` for a reload.
    fn load(&mut self, again: bool) {
        let path = self.state.path.clone();
        let known = stamp(&path);
        let opened = std::fs::read(&path).map_err(|e| e.to_string()).and_then(|bytes| D::open(bytes, &path));
        self.known = known;
        match opened {
            Ok(doc) => {
                self.doc = Some(doc);
                if again {
                    self.state.version += 1;
                }
                self.state.dirty = false;
                self.state.error.clear();
                if again {
                    (self.emit)(Event::Reloaded);
                }
            }
            // a reload that fails keeps the document open as it was
            Err(e) => self.state.error = e,
        }
        self.publish();
    }

    fn save(&mut self) {
        if self.saving.is_some() {
            return;
        }
        let Some(doc) = self.doc.as_mut() else { return };
        let (bytes, to) = match doc.save(&self.state.path) {
            Ok(saved) => saved,
            Err(e) => {
                self.state.error = e;
                return self.publish();
            }
        };
        let version = self.state.version;
        match self.sink {
            Sink::File => {
                match write_atomic(&to, &bytes) {
                    Ok(()) => {
                        self.state.path = to;
                        self.known = stamp(&self.state.path);
                        self.state.dirty = false;
                        self.state.error.clear();
                        (self.emit)(Event::Saved(self.state.path.clone()));
                    }
                    Err(e) => self.state.error = format!("Couldn't save: {e}"),
                }
                self.publish();
            }
            Sink::Broker => {
                self.saving = Some(version);
                self.state.saving = true;
                self.state.path = to.clone();
                self.publish();
                (self.emit)(Event::Upload { bytes, path: to, version });
            }
        }
    }

    /// The file changed outside: a clean document reloads, a dirty one
    /// asks once per change.
    fn watch(&mut self) {
        if self.saving.is_some() || self.state.conflict || self.doc.is_none() {
            return;
        }
        let now = stamp(&self.state.path);
        // gone (mid-rename by another writer, or deleted): wait for it
        if now.is_none() || now == self.known {
            return;
        }
        if self.state.dirty {
            self.state.conflict = true;
            self.known = now;
            self.publish();
        } else {
            self.load(true);
        }
    }
}

/// Writes `bytes` to `path` through a temp file in the same folder and a
/// rename, so the file is never half written.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.rox-save-{}", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        // the file keeps its permissions
        if let Ok(m) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&tmp, m.permissions());
        }
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A text document: `Some(s)` appends, `None` reads it back.
    struct Text(String);

    impl Document for Text {
        type Cmd = Option<String>;
        type Out = String;

        fn open(bytes: Vec<u8>, _: &Path) -> Result<Self, String> {
            String::from_utf8(bytes).map(Text).map_err(|e| e.to_string())
        }

        fn run(&mut self, cmd: Option<String>) -> (Vec<String>, bool) {
            match cmd {
                Some(s) => {
                    self.0.push_str(&s);
                    (Vec::new(), true)
                }
                None => (vec![self.0.clone()], false),
            }
        }

        fn save(&mut self, path: &Path) -> Result<(Vec<u8>, PathBuf), String> {
            Ok((self.0.clone().into_bytes(), path.to_owned()))
        }
    }

    enum Seen {
        State(State),
        Out(String),
        Upload(u64),
        Reloaded,
        Saved,
    }

    fn open(path: &Path, sink: Sink) -> (Session<Text>, Arc<Mutex<Vec<Seen>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let s = Session::<Text>::open(path.to_owned(), sink, move |e| {
            log.lock().unwrap().push(match e {
                Event::State(s) => Seen::State(s),
                Event::Out(o) => Seen::Out(o),
                Event::Upload { version, .. } => Seen::Upload(version),
                Event::Reloaded => Seen::Reloaded,
                Event::Saved(_) => Seen::Saved,
            })
        });
        (s, seen)
    }

    /// Waits (at most 5 s) for an event `f` picks out.
    fn wait<T>(seen: &Arc<Mutex<Vec<Seen>>>, f: impl Fn(&Seen) -> Option<T>) -> T {
        let t = std::time::Instant::now();
        loop {
            if let Some(v) = seen.lock().unwrap().iter().rev().find_map(&f) {
                return v;
            }
            assert!(t.elapsed() < Duration::from_secs(5), "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn last_state(seen: &Arc<Mutex<Vec<Seen>>>) -> State {
        wait(seen, |e| if let Seen::State(s) = e { Some(s.clone()) } else { None })
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rox-office-session-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn edits_version_and_dirty_then_save_is_atomic() {
        let d = dir("save");
        let path = d.join("a.txt");
        std::fs::write(&path, "one").unwrap();
        let (s, seen) = open(&path, Sink::File);
        assert_eq!(last_state(&seen).version, 0);
        s.send(Some(" two".into()));
        wait(&seen, |e| matches!(e, Seen::State(s) if s.version == 1 && s.dirty).then_some(()));
        s.save();
        wait(&seen, |e| matches!(e, Seen::Saved).then_some(()));
        assert!(!last_state(&seen).dirty);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one two");
        // no temp file left behind
        let names: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("a.txt")]);
    }

    #[test]
    fn write_atomic_replaces_and_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("atomic");
        let path = d.join("b.bin");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o640);
        // a folder that isn't there fails and leaves nothing
        assert!(write_atomic(&d.join("nope/c.bin"), b"x").is_err());
    }

    #[test]
    fn outside_change_reloads_a_clean_document() {
        let d = dir("reload");
        let path = d.join("a.txt");
        std::fs::write(&path, "first").unwrap();
        let (s, seen) = open(&path, Sink::File);
        last_state(&seen);
        // another size, so the change shows whatever the clock's grain
        std::fs::write(&path, "second!").unwrap();
        wait(&seen, |e| matches!(e, Seen::Reloaded).then_some(()));
        s.send(None);
        assert_eq!(wait(&seen, |e| if let Seen::Out(o) = e { Some(o.clone()) } else { None }), "second!");
        let st = last_state(&seen);
        assert_eq!((st.version, st.dirty, st.conflict), (1, false, false));
    }

    #[test]
    fn outside_change_to_a_dirty_document_asks() {
        let d = dir("conflict");
        let path = d.join("a.txt");
        std::fs::write(&path, "base").unwrap();
        let (s, seen) = open(&path, Sink::File);
        s.send(Some("+mine".into()));
        wait(&seen, |e| matches!(e, Seen::State(s) if s.dirty).then_some(()));
        std::fs::write(&path, "theirs, longer").unwrap();
        wait(&seen, |e| matches!(e, Seen::State(s) if s.conflict).then_some(()));
        // keeping ours: no reload, and the save overwrites theirs
        s.keep_mine();
        wait(&seen, |e| matches!(e, Seen::State(s) if !s.conflict && s.dirty).then_some(()));
        s.save();
        wait(&seen, |e| matches!(e, Seen::Saved).then_some(()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "base+mine");
        // taking theirs on the next change
        std::fs::write(&path, "theirs again").unwrap();
        s.send(Some("!".into()));
        wait(&seen, |e| matches!(e, Seen::State(s) if s.conflict).then_some(()));
        s.reload();
        wait(&seen, |e| matches!(e, Seen::Reloaded).then_some(()));
        s.send(None);
        assert_eq!(wait(&seen, |e| if let Seen::Out(o) = e { Some(o.clone()) } else { None }), "theirs again");
    }

    #[test]
    fn a_broker_save_waits_for_the_upload() {
        let d = dir("broker");
        let path = d.join("a.txt");
        std::fs::write(&path, "x").unwrap();
        let (s, seen) = open(&path, Sink::Broker);
        s.send(Some("y".into()));
        s.save();
        let v = wait(&seen, |e| if let Seen::Upload(v) = e { Some(*v) } else { None });
        assert_eq!(v, 1);
        assert!(last_state(&seen).saving);
        // an edit during the upload stays unsaved
        s.send(Some("z".into()));
        wait(&seen, |e| matches!(e, Seen::State(s) if s.version == 2).then_some(()));
        // the broker wrote it (as the upload would)
        std::fs::write(&path, "xy").unwrap();
        s.uploaded(1, true, String::new());
        wait(&seen, |e| matches!(e, Seen::Saved).then_some(()));
        let st = last_state(&seen);
        assert!(!st.saving && st.dirty, "{st:?}");
        // the broker's write is not an outside change
        std::thread::sleep(WATCH_EVERY * 2);
        assert!(!seen.lock().unwrap().iter().any(|e| matches!(e, Seen::Reloaded)));
        assert!(!last_state(&seen).conflict);
    }
}
