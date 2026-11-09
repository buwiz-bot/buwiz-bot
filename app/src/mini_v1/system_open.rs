//! `_meta["buwiz/open_with"] = {path}` and `_meta["buwiz/save"] = {path}`: a
//! file a first-party plugin asks the Bot to open in the system's default
//! app, or to save to the person's Downloads folder. Only `path` is read; any
//! other field (a `name`, a `mime`) is ignored: the name and the kind are the
//! file's own on disk, where the plugin named it.
//!
//! Both pass `document`'s checks ([`document::checked`]): first-party, the
//! person's surface and not the AI's, a real file at a plugin's hand-off place
//! (`open-<hex>/<folder>/<file>`) with no link on the way. The bridge also
//! wants the person's own press just before the call, which the hand-off
//! spends (`attribution`'s `fresh_press`).
//!
//! The file is opened once, at the check, and everything after reads from
//! that open file, never the path again: a plugin that swaps the file for a
//! link after the check still gets only what was checked. A save copies it to
//! Downloads. An open copies it to a folder of the Bot's own
//! ([`opened_dir`]) and opens that copy, never the plugin's path.
//!
//! An open also needs a kind on [`SAFE`], by the extension on disk: anything
//! else opens nothing, whatever app the system has for it. A save takes any
//! kind, as a browser's download does.
//!
//! Every copy is marked as downloaded (`com.apple.quarantine`, as a browser
//! does) the moment it exists, before a byte is in it, so Gatekeeper and the
//! app that opens it treat it as a file from the internet: a mail's
//! attachment is a stranger's. The plugin marks what it writes too, but a
//! copy needn't carry that. A name with a character that makes it show as
//! another is refused at the check (`document::deceives`), with the person
//! told, so a copy keeps the name on disk as it is.
//!
//! A refusal carries no part of the path, as `document`'s does ([`Refused`]).

use std::{fs, path::Path, sync::Arc};
#[cfg(target_os = "macos")]
use std::{
    io,
    path::PathBuf,
    time::{Duration, SystemTime},
};

use serde_json::Value;

use super::{
    attribution::Actor,
    document::{self, Refused},
};

/// The kinds `buwiz/open_with` opens, by the extension on disk: pictures,
/// PDFs, text, Office and iWork files, calendar invites. No program, script,
/// disk image, installer or web page, no legacy Office file (`doc`, `xls`,
/// `ppt`), which can carry macros, and no archive, which Archive Utility
/// would unpack beside it, past these checks: a zip is for saving.
pub const SAFE: [&str; 16] = ["png", "jpg", "jpeg", "gif", "webp", "heic", "pdf", "txt", "csv", "docx", "xlsx", "pptx", "pages", "numbers", "key", "ics"];

/// A file that passed [`check_open`] or [`check_save`]: the name it has on
/// disk, and the file, opened at the check. No path: nothing after the check
/// goes by it.
#[derive(Debug, Clone)]
pub struct LocalFile {
    name: String,
    file: Arc<fs::File>,
}

impl LocalFile {
    /// Its name on disk.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Whether `buwiz/open_with`'s `field`, from a call `actor` made to a plugin
/// with data folder `folder`, may open. `first_party` as `document::check`
/// takes it.
pub fn check_open(first_party: bool, actor: Actor, folder: Option<&Path>, field: &Value) -> Result<LocalFile, Refused> {
    let (_, name, file) = document::checked(first_party, actor, folder, field)?;
    if !SAFE.contains(&document::ext(&name).as_str()) {
        return Err(Refused::Kind);
    }
    Ok(LocalFile { name, file: Arc::new(file) })
}

/// Whether `buwiz/save`'s `field` may save: [`check_open`] without the kind.
pub fn check_save(first_party: bool, actor: Actor, folder: Option<&Path>, field: &Value) -> Result<LocalFile, Refused> {
    let (_, name, file) = document::checked(first_party, actor, folder, field)?;
    Ok(LocalFile { name, file: Arc::new(file) })
}

/// `<home>/Downloads`: `None` without a real home.
#[cfg(target_os = "macos")]
pub fn downloads() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    home.is_absolute().then(|| home.join("Downloads"))
}

/// Where the copies a plugin's files are opened and shown from go, one
/// folder per copy, so each keeps its name: the Bot's caches, which no backup
/// keeps, `~/Library/Caches/<bundle id>/opened`. A test instance
/// (`BUWIZ_DATA_DIR`) keeps its own in its data folder, so its sweep never
/// reaches the owner's. `None` without a real home.
#[cfg(target_os = "macos")]
pub fn opened_dir() -> Option<PathBuf> {
    let test = buwiz_protocol::env::var_os("BUWIZ_DATA_DIR");
    opened_dir_of(std::env::var_os("HOME").as_deref(), test.as_deref())
}

/// [`opened_dir`] for `home` and a test instance's data folder `test`, each
/// only if absolute.
#[cfg(target_os = "macos")]
fn opened_dir_of(home: Option<&std::ffi::OsStr>, test: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    match test.filter(|t| !t.is_empty()) {
        Some(dir) => Some(Path::new(dir)).filter(|d| d.is_absolute()).map(|d| d.join("Caches/opened")),
        None => home.map(Path::new).filter(|h| h.is_absolute()).map(|h| h.join("Library/Caches").join(crate::identity::APP_ID).join("opened")),
    }
}

/// How long a copy [`opened_dir`] holds lives: the app it went to has long
/// read it by then.
#[cfg(target_os = "macos")]
pub const OPENED_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// `name` the `n`th time it is there: `IMG_4250 (2).png`.
#[cfg(target_os = "macos")]
fn numbered(name: &str, n: u32) -> String {
    if n < 2 {
        return name.to_owned();
    }
    let p = Path::new(name);
    match (p.file_stem().and_then(|s| s.to_str()), p.extension().and_then(|e| e.to_str())) {
        (Some(stem), Some(ext)) => format!("{stem} ({n}).{ext}"),
        _ => format!("{name} ({n})"),
    }
}

/// A new file at `to`, never one there already nor through a link
/// (`create_new`), marked as downloaded before anything is written to it. A
/// file that can't be marked is taken away again.
#[cfg(target_os = "macos")]
fn create_marked(to: &Path) -> io::Result<fs::File> {
    let out = fs::OpenOptions::new().write(true).create_new(true).open(to)?;
    if !mark_downloaded(to) {
        drop(out);
        let _ = fs::remove_file(to);
        return Err(io::Error::other("not marked"));
    }
    Ok(out)
}

/// Copies all of `from` into `out`, at `to`, which goes again if the copy
/// fails; to the disk itself if `sync` (a save, which the person keeps; a
/// copy to open needn't outlive a crash). It reads by offset from the start, never the file's own position,
/// which every holder of the open file shares: a second copy reads it all
/// too. Past `most` bytes, if any, it fails: a file the plugin keeps
/// writing to after the check grows no copy past the cap ([`Cap`]). `out`
/// back, once whole.
#[cfg(target_os = "macos")]
fn fill(from: &fs::File, mut out: fs::File, to: &Path, sync: bool, most: Option<u64>) -> io::Result<fs::File> {
    use std::{io::Write, os::unix::fs::FileExt};
    let mut copy = || -> io::Result<()> {
        let mut buf = vec![0; 1 << 16];
        let mut at = 0;
        loop {
            let n = from.read_at(&mut buf, at)?;
            if n == 0 {
                return if sync { out.sync_all() } else { Ok(()) };
            }
            at += n as u64;
            if most.is_some_and(|most| at > most) {
                return Err(io::ErrorKind::FileTooLarge.into());
            }
            out.write_all(&buf[..n])?;
        }
    };
    match copy() {
        Ok(()) => Ok(out),
        Err(e) => {
            drop(out);
            let _ = fs::remove_file(to);
            Err(e)
        }
    }
}

/// Copies `file` into `dir` under its own name, cleaned, or the first
/// numbered one free there: the path written. Nothing there is ever written
/// over, a link included.
#[cfg(target_os = "macos")]
pub fn save_in(file: &LocalFile, dir: &Path) -> io::Result<PathBuf> {
    for n in 1..=999 {
        let to = dir.join(numbered(&file.name, n));
        let out = match create_marked(&to) {
            Ok(out) => out,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        return fill(&file.file, out, &to, true, None).map(|_| to);
    }
    Err(io::Error::from(io::ErrorKind::AlreadyExists))
}

/// A folder only this account may enter, made if it isn't there.
#[cfg(target_os = "macos")]
fn private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// Copies `file` for `buwiz/open_with` into a new folder of its own in
/// `base`, under its own name, within `cap`: the copy's path, for
/// [`open_copy`].
#[cfg(target_os = "macos")]
pub fn copy_for_open(file: &LocalFile, base: &Path, cap: Cap) -> io::Result<PathBuf> {
    copy_file(&file.file, &file.name, base, None, cap)
}

/// A copy in [`opened_dir`]: where it is, and whether this hand-off made it
/// (a viewer's copy can be one an earlier open made, [`copy_for_view`]).
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copied {
    pub path: PathBuf,
    pub fresh: bool,
}

impl Copied {
    /// A copy this hand-off made.
    #[cfg(target_os = "macos")]
    pub fn fresh(path: PathBuf) -> Copied {
        Copied { path, fresh: true }
    }
}

/// What the Bot writes down of each copy it makes, beside its folder in
/// [`opened_dir`] (`<id>.copy`): the copy's name and inode, all a sweep may
/// take away, and for a viewer's copy the file it came from.
///
/// It is written first, `pending`, before the folder is made, and again
/// once the copy is whole: a copy cut off midway (the Bot killed, the disk
/// full) is still on record, so the sweep takes it. A pending copy was never
/// handed to anything, and its folder was only ever the Bot's: the file its
/// record names is taken by that name, as a regular file, with no inode yet.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct Record {
    name: String,
    ino: Option<u64>,
    bytes: u64,
    /// The checked file's device, inode, change time (ns) and size.
    source: Option<[u64; 4]>,
    pending: bool,
}

#[cfg(target_os = "macos")]
impl Record {
    fn to_json(&self) -> String {
        serde_json::json!({"name": self.name, "ino": self.ino, "bytes": self.bytes, "source": self.source, "pending": self.pending}).to_string()
    }

    /// The record at `at`, read through no link and never waiting on a pipe
    /// put in its place ([`open_plain`]).
    fn read(at: &Path) -> Option<Record> {
        use std::io::Read;
        let mut text = String::new();
        open_plain(at)?.take(1 << 16).read_to_string(&mut text).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        let source = v.get("source").and_then(Value::as_array).and_then(|a| {
            let n: Vec<u64> = a.iter().filter_map(Value::as_u64).collect();
            <[u64; 4]>::try_from(n).ok()
        });
        let pending = v.get("pending").and_then(Value::as_bool).unwrap_or(false);
        let ino = v.get("ino").and_then(Value::as_u64);
        if ino.is_none() && !pending {
            return None;
        }
        // one plain name in the copy's folder, never a way out of it
        let name = v.get("name")?.as_str()?;
        if !matches!(Path::new(name).components().collect::<Vec<_>>().as_slice(), [std::path::Component::Normal(n)] if *n == name) {
            return None;
        }
        Some(Record { name: name.to_owned(), ino, bytes: v.get("bytes").and_then(Value::as_u64).unwrap_or(0), source, pending })
    }

    /// Writes this down as copy `id`'s in `base`: a `new` record never over
    /// another, and never through a link or into a pipe.
    fn write(&self, base: &Path, id: &str, new: bool) -> io::Result<()> {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let mut opts = fs::OpenOptions::new();
        opts.write(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        if new {
            opts.create_new(true);
        } else {
            opts.truncate(true);
        }
        let mut f = opts.open(base.join(format!("{id}.copy")))?;
        if !f.metadata()?.file_type().is_file() {
            return Err(io::Error::other("not a record"));
        }
        f.write_all(self.to_json().as_bytes())
    }
}

/// `path` opened to read, never through a link (`O_NOFOLLOW`) and never
/// waiting on a pipe put in its place (`O_NONBLOCK`): `None` unless it is a
/// regular file.
#[cfg(target_os = "macos")]
fn open_plain(path: &Path) -> Option<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let f = fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path).ok()?;
    f.metadata().ok()?.file_type().is_file().then_some(f)
}

/// Marks the record at `at` as used now, through the record itself
/// ([`open_plain`]): a link or a pipe in its place is left as it is.
#[cfg(target_os = "macos")]
fn touch(at: &Path) -> io::Result<()> {
    open_plain(at).ok_or_else(|| io::Error::other("not a record"))?.set_modified(SystemTime::now())
}

/// The folder at `dir`, opened as itself, never through a link, its mode set
/// through that (`fchmod`): the open folder, for what is done in it next.
#[cfg(target_os = "macos")]
fn chmod_dir(dir: &Path, mode: u32) -> io::Result<fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let d = fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY).open(dir)?;
    d.set_permissions(fs::Permissions::from_mode(mode))?;
    Ok(d)
}

/// The identity of an open file, for a viewer's copy to be reused by: its
/// device, inode, modification time (ns) and size.
#[cfg(target_os = "macos")]
fn identity(from: &fs::File) -> io::Result<[u64; 4]> {
    use std::os::unix::fs::MetadataExt;
    let m = from.metadata()?;
    Ok([m.dev(), m.ino(), (m.mtime() as u64).wrapping_mul(1_000_000_000).wrapping_add(m.mtime_nsec() as u64), m.size()])
}

/// The most copies [`opened_dir`] keeps, and the most bytes in them: past
/// either, the oldest go first ([`cap`]). A single file over `bytes` gets no
/// copy at all.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cap {
    pub most: usize,
    pub bytes: u64,
}

/// The Bot's [`Cap`]: 64 copies, 1 GiB.
#[cfg(target_os = "macos")]
pub const CAP: Cap = Cap { most: 64, bytes: 1 << 30 };

/// Copies all of `from`, named `name` on disk, into a new folder of its own
/// in `base`, under that name, marked, then locked: the file read-only, its
/// folder too, so the app it opens in shows it locked and a Duplicate or a
/// Save As can't land in it, where the sweep would take it. The copy is
/// written down ([`Record`]) before anything else is made, with `source` for
/// a viewer's. A copy that fails takes its folder with it. A file over
/// `cap`'s bytes is refused before anything is written; after a copy, the
/// oldest others go past `cap` ([`cap`]), never this one.
#[cfg(target_os = "macos")]
pub fn copy_file(from: &fs::File, name: &str, base: &Path, source: Option<[u64; 4]>, cap: Cap) -> io::Result<PathBuf> {
    copy_with(name, base, source, cap, |out, to| fill(from, out, to, false, Some(cap.bytes)), from.metadata()?.len())
}

/// [`copy_file`], the bytes put in by `put` (into the new file, at its path,
/// the file back once whole), of a file `size` long.
#[cfg(target_os = "macos")]
fn copy_with(name: &str, base: &Path, source: Option<[u64; 4]>, cap: Cap, put: impl FnOnce(fs::File, &Path) -> io::Result<fs::File>, size: u64) -> io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    if size > cap.bytes {
        return Err(io::ErrorKind::FileTooLarge.into());
    }
    private_dir(base)?;
    // room first, from copies no app is likely to hold open; none, no copy
    if !self::cap(base, cap, None, Some(size)) {
        return Err(io::ErrorKind::FileTooLarge.into());
    }
    let id = super::keys::random_id();
    // the size it will be: a copy cut off midway counts against the cap
    let mut record = Record { name: name.to_owned(), ino: None, bytes: size, source, pending: true };
    // on record before the folder is: a copy cut off midway is the sweep's
    record.write(base, &id, true)?;
    let dir = base.join(&id);
    let to = dir.join(name);
    let made = || -> io::Result<()> {
        fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let out = put(create_marked(&to)?, &to)?;
        out.set_permissions(fs::Permissions::from_mode(0o444))?;
        let m = out.metadata()?;
        record = Record { ino: Some(m.ino()), bytes: m.size(), pending: false, ..record.clone() };
        record.write(base, &id, false)?;
        chmod_dir(&dir, 0o555).map(drop)
    };
    if let Err(e) = made() {
        remove_copy(base, &id, &record);
        return Err(e);
    }
    self::cap(base, cap, Some(&id), None);
    Ok(to)
}

/// A copy of `from` for the viewer, named `name`: the one an earlier open of
/// the very same file made, if it is still whole, or a new one, within `cap`
/// ([`copy_file`]).
#[cfg(target_os = "macos")]
pub fn copy_for_view(from: &fs::File, name: &str, base: &Path, cap: Cap) -> io::Result<Copied> {
    use std::os::unix::fs::MetadataExt;
    let source = identity(from)?;
    for (id, record, _) in records(base) {
        let path = base.join(&id).join(&record.name);
        let whole = fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file() && Some(m.ino()) == record.ino);
        if record.source == Some(source) && record.name == name && whole && !record.pending {
            // in use again: the sweep counts its day from now
            let _ = touch(&base.join(format!("{id}.copy")));
            return Ok(Copied { path, fresh: false });
        }
    }
    copy_file(from, name, base, Some(source), cap).map(Copied::fresh)
}

/// The copies `base` has written down: each one's id, record, and when it
/// was last made or used.
#[cfg(target_os = "macos")]
fn records(base: &Path) -> Vec<(String, Record, SystemTime)> {
    let Ok(entries) = fs::read_dir(base) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|n| n.strip_suffix(".copy")) else { continue };
        let Ok(meta) = fs::symlink_metadata(entry.path()) else { continue };
        if !meta.file_type().is_file() {
            continue;
        }
        if let (Some(record), Ok(at)) = (Record::read(&entry.path()), meta.modified()) {
            out.push((id.to_owned(), record, at));
        }
    }
    out
}

/// Takes away the copy `id` of `base`: its folder unlocked, the file its
/// record names if it is still that file (a regular one, the same inode, or
/// any inode while the copy is `pending`), the folder if that leaves it
/// empty, then the record. All of it through the folder opened as itself,
/// never a link ([`chmod_dir`]): nothing else in the folder is touched, and
/// nothing is followed or gone into.
#[cfg(target_os = "macos")]
fn remove_copy(base: &Path, id: &str, record: &Record) {
    use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};
    let dir = base.join(id);
    let gone = |at: &Path| at.symlink_metadata().is_err_and(|e| e.kind() == io::ErrorKind::NotFound);
    if gone(&dir) {
        let _ = fs::remove_file(base.join(format!("{id}.copy")));
        return;
    }
    let Ok(open) = chmod_dir(&dir, 0o700) else { return };
    if let Ok(name) = std::ffi::CString::new(std::ffi::OsStr::new(&record.name).as_bytes()) {
        let fd = open.as_raw_fd();
        // SAFETY: fstatat and unlinkat on the open folder, with a
        // NUL-terminated name and a stat buffer of our own; no link followed.
        unsafe {
            let mut st: libc::stat = std::mem::zeroed();
            let found = libc::fstatat(fd, name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) == 0;
            let file = found && (st.st_mode & libc::S_IFMT) == libc::S_IFREG;
            if file && (record.pending || record.ino == Some(st.st_ino)) {
                libc::unlinkat(fd, name.as_ptr(), 0);
            }
        }
    }
    drop(open);
    // only if empty: a stranger's file keeps the folder
    let _ = fs::remove_dir(&dir);
    if gone(&dir) {
        let _ = fs::remove_file(base.join(format!("{id}.copy")));
    }
}

/// Takes away the copy at `copy` this hand-off made ([`remove_copy`]).
#[cfg(target_os = "macos")]
pub fn discard(copy: &Path) {
    let (Some(dir), Some(name)) = (copy.parent(), copy.file_name()) else { return };
    let (Some(base), Some(id)) = (dir.parent(), dir.file_name().and_then(|i| i.to_str())) else { return };
    if let Some(record) = Record::read(&base.join(format!("{id}.copy"))).filter(|r| name == r.name.as_str()) {
        remove_copy(base, id, &record);
    }
}

/// How long a copy counts as in use after it was made or last used: an app
/// may have it open, so the cap never takes it ([`cap`]).
#[cfg(target_os = "macos")]
pub const IN_USE: Duration = Duration::from_secs(10 * 60);

/// Keeps `base` to `cap`'s copies and bytes, with room for one more of
/// `coming` bytes if any, taking the oldest first, but never `keep` (the
/// copy a hand-off is returning), a copy made or used within [`IN_USE`],
/// nor one still being made (pending, and under a day old). Whether it
/// fits. For a copy `coming`, it takes nothing unless that makes room: a
/// copy refused leaves the others be.
#[cfg(target_os = "macos")]
pub fn cap(base: &Path, cap: Cap, keep: Option<&str>, coming: Option<u64>) -> bool {
    let now = SystemTime::now();
    let mut all = records(base);
    all.sort_by_key(|(_, _, at)| *at);
    let mut total: u64 = all.iter().map(|(_, r, _)| r.bytes).sum::<u64>() + coming.unwrap_or(0);
    let mut left = all.len() + usize::from(coming.is_some());
    let mut take = Vec::new();
    for (id, record, at) in &all {
        if left <= cap.most && total <= cap.bytes {
            break;
        }
        let age = now.duration_since(*at).unwrap_or_default();
        let making = record.pending && age <= OPENED_FOR;
        if keep == Some(id.as_str()) || making || age < IN_USE {
            continue;
        }
        take.push((id, record));
        total = total.saturating_sub(record.bytes);
        left -= 1;
    }
    let fits = left <= cap.most && total <= cap.bytes;
    if fits || coming.is_none() {
        for (id, record) in take {
            remove_copy(base, id, record);
        }
    }
    fits
}

/// How a hand-off that copies went ([`copy_between_checks`]).
#[cfg(target_os = "macos")]
#[derive(Debug, PartialEq, Eq)]
pub enum Handed {
    /// The screen wasn't the person's any more before the copy: none made.
    LeftBefore,
    /// The copy failed, and took its folder.
    Failed(io::ErrorKind),
    /// The screen wasn't the person's any more once the copy was there: a
    /// copy this hand-off made went again.
    LeftAfter,
    /// Handed on to `done`.
    Done,
}

/// A hand-off that copies, in its order: `shows()` (the open still the
/// person's, on top), the copy, `shows()` again, then `done` with the copy. A
/// copy can take a while, and the person can leave meanwhile: a miss after it
/// takes a copy this hand-off made away again.
#[cfg(target_os = "macos")]
pub fn copy_between_checks(shows: &dyn Fn() -> bool, copy: impl FnOnce() -> io::Result<Copied>, done: impl FnOnce(Copied)) -> Handed {
    if !shows() {
        return Handed::LeftBefore;
    }
    let copy = match copy() {
        Ok(copy) => copy,
        Err(e) => return Handed::Failed(e.kind()),
    };
    if !shows() {
        if copy.fresh {
            discard(&copy.path);
        }
        return Handed::LeftAfter;
    }
    done(copy);
    Handed::Done
}

/// The last step of `buwiz/open_with`, on the main thread: asks `on_top`
/// (the open still the person's screen, on top, and live) in this very step,
/// then `open`s the copy if so, and takes the copy away if not or nothing
/// opened it. Whether it opened.
#[cfg(target_os = "macos")]
pub fn open_or_discard(copy: &Path, on_top: impl FnOnce() -> bool, open: impl FnOnce(&Path) -> bool) -> bool {
    let opened = on_top() && open(copy);
    if !opened {
        discard(copy);
    }
    opened
}

/// Takes away the copies in `base` last made or used before [`OPENED_FOR`]
/// at `now`, a copy cut off midway among them: only what each record names
/// ([`remove_copy`]). Then keeps the rest to `cap` ([`cap`]).
#[cfg(target_os = "macos")]
pub fn sweep(base: &Path, now: SystemTime, cap: Cap) {
    for (id, record, at) in records(base) {
        if now.duration_since(at).is_ok_and(|age| age > OPENED_FOR) {
            remove_copy(base, &id, &record);
        }
    }
    self::cap(base, cap, None, None);
}

/// How often the Bot sweeps [`opened_dir`] while it runs.
#[cfg(target_os = "macos")]
pub const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);

/// [`sweep`]s `base` now and every `every` after, for good: a thread's whole
/// work.
#[cfg(target_os = "macos")]
pub fn sweep_every(base: &Path, every: Duration) {
    loop {
        sweep(base, SystemTime::now(), CAP);
        std::thread::sleep(every);
    }
}

/// [`sweep_every`] of [`opened_dir`], from launch, hourly: on a thread of its
/// own.
#[cfg(target_os = "macos")]
pub fn sweep_while_running() {
    if let Some(base) = opened_dir() {
        sweep_every(&base, SWEEP_EVERY);
    }
}

/// Marks `path` as a browser's download (`com.apple.quarantine`), through
/// the URL resource key that lets the system write the flags: `false` when
/// it couldn't.
#[cfg(target_os = "macos")]
pub fn mark_downloaded(path: &Path) -> bool {
    use nsh_platform_macos::{
        objc2::{msg_send, rc::Retained, runtime::AnyObject},
        objc2_foundation::{NSDictionary, NSString, NSURL},
    };
    let Some(path) = path.to_str() else { return false };
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    let (kind, agent) = (NSString::from_str("LSQuarantineType"), NSString::from_str("LSQuarantineAgentName"));
    let (download, bot) = (NSString::from_str("LSQuarantineTypeWebDownload"), NSString::from_str("Buwiz Bot"));
    let props: Retained<NSDictionary<NSString, NSString>> = NSDictionary::from_slices(&[&*kind, &*agent], &[&*download, &*bot]);
    let key = NSString::from_str("NSURLQuarantinePropertiesKey");
    // SAFETY: NSURL's documented setResourceValue:forKey:error:, with a
    // dictionary of the documented keys and no error out-pointer.
    unsafe { msg_send![&*url, setResourceValue: &*props, forKey: &*key, error: std::ptr::null_mut::<*mut AnyObject>()] }
}

/// Whether `path` carries the downloaded mark.
#[cfg(target_os = "macos")]
pub fn is_marked(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: getxattr with NUL-terminated strings and no buffer: it only
    // answers the size.
    unsafe { libc::getxattr(path.as_ptr(), c"com.apple.quarantine".as_ptr(), std::ptr::null_mut(), 0, 0, libc::XATTR_NOFOLLOW) >= 0 }
}

/// Opens a copy [`copy_for_open`] made in the system's default app for its
/// kind, through NSWorkspace as `native::open_in_browser` opens a URL, if it
/// is still marked. On the main thread.
#[cfg(target_os = "macos")]
pub fn open_copy(copy: &Path) -> bool {
    use nsh_platform_macos::{
        objc2::{
            msg_send,
            rc::Retained,
            runtime::{AnyClass, AnyObject},
        },
        objc2_foundation::{NSString, NSURL},
    };
    let (true, Some(path), Some(class)) = (is_marked(copy), copy.to_str(), AnyClass::get(c"NSWorkspace")) else { return false };
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    // SAFETY: NSWorkspace's documented methods, on the main thread.
    unsafe {
        let ws: Retained<AnyObject> = msg_send![class, sharedWorkspace];
        msg_send![&*ws, openURL: &*url]
    }
}

#[cfg(test)]
mod tests {
    use std::{os::unix::fs::symlink, path::PathBuf};

    use serde_json::json;

    use super::*;

    /// A plugin's folder with a hand-off folder `open-1f/1/` in it, a folder
    /// beside it, and a Downloads folder.
    struct Bed {
        base: PathBuf,
        folder: PathBuf,
    }

    impl Bed {
        fn new() -> Bed {
            let base = crate::mini_v1::storage::scratch_root("system-open");
            let folder = base.join("app.buwiz.fixture");
            fs::create_dir_all(folder.join("open-1f/1")).unwrap();
            fs::create_dir_all(base.join("elsewhere")).unwrap();
            fs::create_dir_all(base.join("Downloads")).unwrap();
            Bed { base, folder }
        }

        fn file(&self, name: &str) -> PathBuf {
            let path = self.folder.join("open-1f/1").join(name);
            fs::write(&path, name).unwrap();
            path
        }

        fn outside(&self, name: &str) -> PathBuf {
            let path = self.base.join("elsewhere").join(name);
            fs::write(&path, "elsewhere").unwrap();
            path
        }

        fn open(&self, path: &Path) -> Result<LocalFile, Refused> {
            check_open(true, Actor::Person, Some(&self.folder), &json!({"path": path}))
        }

        fn save(&self, path: &Path) -> Result<LocalFile, Refused> {
            check_save(true, Actor::Person, Some(&self.folder), &json!({"path": path, "name": "x.png"}))
        }

        #[cfg(target_os = "macos")]
        fn downloads(&self) -> PathBuf {
            self.base.join("Downloads")
        }

        #[cfg(target_os = "macos")]
        fn opened(&self) -> PathBuf {
            self.base.join("opened")
        }
    }

    impl Drop for Bed {
        fn drop(&mut self) {
            // the copies' folders are locked
            use std::os::unix::fs::PermissionsExt;
            for entry in fs::read_dir(self.base.join("opened")).into_iter().flatten().flatten() {
                let _ = fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o700));
            }
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    /// Makes the copy at `copy` last made or used `ago`.
    #[cfg(target_os = "macos")]
    fn age(copy: &Path, ago: Duration) {
        let dir = copy.parent().unwrap();
        let record = dir.parent().unwrap().join(format!("{}.copy", dir.file_name().unwrap().to_str().unwrap()));
        fs::File::open(record).unwrap().set_modified(SystemTime::now() - ago).unwrap();
    }

    #[cfg(target_os = "macos")]
    fn mode(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn every_kind_on_the_safe_list_opens_by_its_name_on_disk() {
        let bed = Bed::new();
        for ext in SAFE {
            let name = format!("a.{ext}");
            assert_eq!(bed.open(&bed.file(&name)).unwrap().name(), name);
            let loud = format!("B.{}", ext.to_uppercase());
            assert_eq!(bed.open(&bed.file(&loud)).unwrap().name(), loud);
        }
    }

    #[test]
    fn a_kind_off_the_list_opens_nothing() {
        let bed = Bed::new();
        let off = [
            "run.exe", "a.app", "a.command", "a.sh", "a.html", "a.htm", "a.svg", "a.dmg", "a.pkg", "a.jar", "a.scpt", "a.terminal", "a.webloc", "a.png.exe", "a.pdf.command",
            "a.doc", "a.xls", "a.ppt", "a.docm", "a.xlsm", "a.zip", "noextension", ".png", "a.png ", "a.",
        ];
        for name in off {
            assert_eq!(bed.open(&bed.file(name)).err(), Some(Refused::Kind), "{name}");
        }
    }

    #[test]
    fn a_zip_saves_where_it_may_not_open() {
        let bed = Bed::new();
        let zip = bed.file("Photos.zip");
        assert_eq!(bed.open(&zip).err(), Some(Refused::Kind));
        assert_eq!(bed.save(&zip).unwrap().name(), "Photos.zip");
    }

    #[test]
    fn a_kelvin_sign_cant_pass_for_the_k_of_key() {
        let bed = Bed::new();
        assert_eq!(bed.open(&bed.file("x.\u{212A}EY")).err(), Some(Refused::Kind));
        // another name: the disk folds case, and takes the Kelvin sign for a k
        assert!(bed.open(&bed.file("y.KEY")).is_ok());
    }

    #[test]
    fn a_png_name_cant_launder_an_exe() {
        let bed = Bed::new();
        let field = json!({"path": bed.file("run.exe"), "name": "IMG_4250.png", "mime": "image/png"});
        assert_eq!(check_open(true, Actor::Person, Some(&bed.folder), &field).err(), Some(Refused::Kind));
        let saved = check_save(true, Actor::Person, Some(&bed.folder), &field).unwrap();
        assert_eq!(saved.name(), "run.exe", "a save keeps the name on disk, not the field's");
    }

    #[test]
    fn the_ais_call_or_a_third_party_plugin_opens_and_saves_nothing() {
        let bed = Bed::new();
        let field = json!({"path": bed.file("a.png")});
        for check in [check_open, check_save] {
            assert_eq!(check(false, Actor::Person, Some(&bed.folder), &field).err(), Some(Refused::ThirdParty));
            assert_eq!(check(true, Actor::Agent, Some(&bed.folder), &field).err(), Some(Refused::NotThePerson));
            assert_eq!(check(true, Actor::Person, None, &field).err(), Some(Refused::NoFolder));
            assert_eq!(check(true, Actor::Person, Some(&bed.folder), &json!({"name": "a.png"})).err(), Some(Refused::BadField));
        }
    }

    #[test]
    fn a_link_or_a_file_outside_the_folder_opens_and_saves_nothing() {
        let bed = Bed::new();
        let outside = bed.outside("a.png");
        let link = bed.folder.join("open-1f/1/link.png");
        symlink(&outside, &link).unwrap();
        symlink(bed.base.join("elsewhere"), bed.folder.join("open-1f/2")).unwrap();
        let hard = bed.folder.join("open-1f/1/hard.png");
        fs::hard_link(bed.outside("b.png"), &hard).unwrap();
        for (path, why) in [
            (outside.clone(), Refused::Outside),
            (bed.folder.join("../elsewhere/a.png"), Refused::Outside),
            (link, Refused::Link),
            (bed.folder.join("open-1f/2/a.png"), Refused::Link),
            (hard, Refused::Link),
            (bed.folder.join("open-1f/1/gone.png"), Refused::Missing),
            (bed.folder.join("open-1f/1"), Refused::NotAFile),
        ] {
            assert_eq!(bed.open(&path).err(), Some(why), "{}", path.display());
            assert_eq!(bed.save(&path).err(), Some(why), "{}", path.display());
        }
    }

    #[test]
    fn only_a_file_in_a_hand_off_folder_opens_or_saves() {
        let bed = Bed::new();
        for rel in ["a.png", "store/a.png", "open-1f/a.png", "open-1f/1/2/a.png", "open-zz/1/a.png"] {
            let path = bed.folder.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "x").unwrap();
            assert_eq!(bed.open(&path).err(), Some(Refused::Shape), "{rel}");
            assert_eq!(bed.save(&path).err(), Some(Refused::Shape), "{rel}");
        }
    }

    #[test]
    fn a_name_that_would_show_as_another_opens_and_saves_nothing() {
        let bed = Bed::new();
        for name in ["invoice\u{202E}fdp.command", "a\u{200B}.pdf", "a\u{2069}.png", "a\u{0007}.pdf"] {
            let path = bed.file(name);
            assert_eq!(bed.open(&path).err(), Some(Refused::Name), "{name:?}");
            assert_eq!(bed.save(&path).err(), Some(Refused::Name), "{name:?}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn copies_go_to_the_bots_caches_not_its_backed_up_data() {
        let home = std::ffi::OsStr::new("/Users/someone");
        let caches = Some(PathBuf::from("/Users/someone/Library/Caches/com.buwiz.bot/opened"));
        assert_eq!(opened_dir_of(Some(home), None), caches);
        assert_eq!(opened_dir_of(Some(home), Some(std::ffi::OsStr::new(""))), caches);
        let test = std::ffi::OsStr::new("/tmp/bench/data");
        assert_eq!(opened_dir_of(Some(home), Some(test)), Some(PathBuf::from("/tmp/bench/data/Caches/opened")), "a test instance's own");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_home_that_isnt_absolute_has_no_copies_folder() {
        for home in ["", "relative", "./x"] {
            assert_eq!(opened_dir_of(Some(std::ffi::OsStr::new(home)), None), None, "{home:?}");
        }
        assert_eq!(opened_dir_of(None, None), None);
        assert_eq!(opened_dir_of(None, Some(std::ffi::OsStr::new("data"))), None, "a test folder too");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_hand_off_checks_the_screen_before_and_after_its_copy() {
        use std::cell::Cell;
        let bed = Bed::new();
        let file = bed.open(&bed.file("a.pdf")).unwrap();
        let answers = |a: &'static [bool]| {
            let at = Cell::new(0);
            move || {
                let i = at.get();
                at.set(i + 1);
                a.get(i).copied().unwrap_or(false)
            }
        };
        let made = Cell::new(false);
        let copy = || {
            made.set(true);
            copy_for_open(&file, &bed.opened(), CAP).map(Copied::fresh)
        };
        assert_eq!(copy_between_checks(&answers(&[false]), copy, |_| panic!("not handed")), Handed::LeftBefore);
        assert!(!made.get(), "no copy for a screen already left");
        let copy = || copy_for_open(&file, &bed.opened(), CAP).map(Copied::fresh);
        assert_eq!(copy_between_checks(&answers(&[true, false]), copy, |_| panic!("not handed")), Handed::LeftAfter);
        assert_eq!(fs::read_dir(bed.opened()).unwrap().count(), 0, "the copy went again");
        let got = Cell::new(None);
        let copy = || copy_for_open(&file, &bed.opened(), CAP).map(Copied::fresh);
        assert_eq!(copy_between_checks(&answers(&[true, true]), copy, |c| got.set(Some(c))), Handed::Done);
        assert_eq!(fs::read(got.take().unwrap().path).unwrap(), b"a.pdf");
        // a viewer's copy an earlier open made stays for that one
        let view = file.file.as_ref();
        let shared = copy_for_view(view, "a.pdf", &bed.opened(), CAP).unwrap();
        let copy = || copy_for_view(view, "a.pdf", &bed.opened(), CAP);
        assert_eq!(copy_between_checks(&answers(&[true, false]), copy, |_| panic!("not handed")), Handed::LeftAfter);
        assert!(shared.path.exists());
    }

    #[cfg(target_os = "macos")]
    fn quarantine(path: &Path) -> Option<String> {
        let out = std::process::Command::new("/usr/bin/xattr").args(["-p", "com.apple.quarantine"]).arg(path).output().unwrap();
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_save_lands_in_downloads_under_its_own_name_marked_as_downloaded() {
        let bed = Bed::new();
        let path = bed.file("IMG_4250.png");
        let file = bed.save(&path).unwrap();
        let to = save_in(&file, &bed.downloads()).unwrap();
        assert_eq!(to, bed.downloads().join("IMG_4250.png"));
        assert_eq!(fs::read(&to).unwrap(), b"IMG_4250.png");
        assert!(quarantine(&to).is_some(), "marked");
        assert!(is_marked(&to));
        assert_eq!(quarantine(&path), None, "the plugin's own file is left as it was");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_save_never_writes_over_a_file_or_through_a_link() {
        let bed = Bed::new();
        let file = bed.save(&bed.file("IMG_4250.png")).unwrap();
        let there = bed.downloads().join("IMG_4250.png");
        fs::write(&there, "the person's own").unwrap();
        // a link where the second name would go, to a file elsewhere
        let target = bed.outside("target.png");
        symlink(&target, bed.downloads().join("IMG_4250 (2).png")).unwrap();
        let to = save_in(&file, &bed.downloads()).unwrap();
        assert_eq!(to, bed.downloads().join("IMG_4250 (3).png"));
        assert_eq!(fs::read(&there).unwrap(), b"the person's own");
        assert_eq!(fs::read(&target).unwrap(), b"elsewhere");
        let file = bed.save(&bed.file("IMG_4250.png")).unwrap();
        assert_eq!(save_in(&file, &bed.downloads()).unwrap(), bed.downloads().join("IMG_4250 (4).png"));
    }

    /// The swap the check can't see: the file goes, a link to a secret takes
    /// its name. What was checked is what is copied.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_file_swapped_for_a_link_after_the_check_copies_what_was_checked() {
        let bed = Bed::new();
        let secret = bed.outside("id_rsa");
        let path = bed.file("IMG_4250.png");
        let save = bed.save(&path).unwrap();
        let open = bed.open(&path).unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&secret, &path).unwrap();
        assert_eq!(fs::read(save_in(&save, &bed.downloads()).unwrap()).unwrap(), b"IMG_4250.png");
        let copy = copy_for_open(&open, &bed.opened(), CAP).unwrap();
        assert_eq!(fs::read(&copy).unwrap(), b"IMG_4250.png");
        assert!(!copy.starts_with(&bed.folder), "the app gets the Bot's copy, never the plugin's path");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_copy_is_marked_before_a_byte_is_in_it() {
        let bed = Bed::new();
        let to = bed.downloads().join("a.pdf");
        let out = create_marked(&to).unwrap();
        assert_eq!(out.metadata().unwrap().len(), 0);
        assert!(quarantine(&to).is_some(), "a kill now leaves a marked, empty file");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_open_gets_a_marked_copy_in_a_folder_only_this_account_enters() {
        let bed = Bed::new();
        let file = bed.open(&bed.file("Invoice.pdf")).unwrap();
        let copy = copy_for_open(&file, &bed.opened(), CAP).unwrap();
        assert_eq!(copy.file_name().unwrap(), "Invoice.pdf");
        assert!(quarantine(&copy).is_some() && is_marked(&copy));
        assert_eq!(mode(&bed.opened()), 0o700);
        assert_eq!(mode(copy.parent().unwrap()), 0o555, "locked: nothing lands beside it");
        assert_eq!(mode(&copy), 0o444, "read-only: the app opens it locked");
        assert!(fs::write(copy.parent().unwrap().join("Invoice copy.pdf"), "x").is_err(), "no Duplicate lands in it");
        // a second open of the same name gets a folder of its own, and all of the file
        let again = copy_for_open(&file, &bed.opened(), CAP).unwrap();
        assert!(again != copy && again.file_name().unwrap() == "Invoice.pdf");
        assert_eq!(fs::read(&copy).unwrap(), b"Invoice.pdf");
        assert_eq!(fs::read(&again).unwrap(), b"Invoice.pdf", "the second copy reads from the start too");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_save_and_an_open_of_one_checked_file_both_get_all_of_it() {
        let bed = Bed::new();
        let file = bed.save(&bed.file("Invoice.pdf")).unwrap();
        let twin = file.clone();
        assert_eq!(fs::read(save_in(&file, &bed.downloads()).unwrap()).unwrap(), b"Invoice.pdf");
        assert_eq!(fs::read(save_in(&twin, &bed.downloads()).unwrap()).unwrap(), b"Invoice.pdf");
        assert_eq!(fs::read(copy_for_open(&twin, &bed.opened(), CAP).unwrap()).unwrap(), b"Invoice.pdf");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_copy_that_fails_leaves_no_folder_behind() {
        let bed = Bed::new();
        let path = bed.file("a.pdf");
        // a file that can't be read from fails the copy after its folder is made
        let unreadable = fs::OpenOptions::new().write(true).open(&path).unwrap();
        assert!(copy_file(&unreadable, "a.pdf", &bed.opened(), None, CAP).is_err());
        assert_eq!(fs::read_dir(bed.opened()).unwrap().count(), 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_sweep_runs_again_while_the_bot_runs() {
        let bed = Bed::new();
        let base = bed.opened();
        fs::create_dir_all(&base).unwrap();
        let swept = base.clone();
        std::thread::spawn(move || sweep_every(&swept, Duration::from_millis(20)));
        // a copy made after the first sweep, a day old by its time
        std::thread::sleep(Duration::from_millis(60));
        let file = bed.open(&bed.file("a.pdf")).unwrap();
        let old = copy_for_open(&file, &base, CAP).unwrap();
        age(&old, OPENED_FOR + Duration::from_secs(60));
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while old.exists() {
            assert!(std::time::Instant::now() < deadline, "no later sweep took it");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(SWEEP_EVERY <= Duration::from_secs(60 * 60), "at least hourly");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_sweep_takes_an_old_copy_and_leaves_a_fresh_one() {
        let bed = Bed::new();
        let file = bed.open(&bed.file("a.pdf")).unwrap();
        let old = copy_for_open(&file, &bed.opened(), CAP).unwrap();
        let fresh = copy_for_open(&file, &bed.opened(), CAP).unwrap();
        age(&old, OPENED_FOR + Duration::from_secs(60));
        sweep(&bed.opened(), SystemTime::now(), CAP);
        assert!(!old.parent().unwrap().exists(), "older than a day, its folder too");
        assert!(fresh.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_sweep_takes_only_its_own_file_and_keeps_a_strangers() {
        use std::os::unix::fs::PermissionsExt;
        let bed = Bed::new();
        let file = bed.open(&bed.file("a.pages")).unwrap();
        let copy = copy_for_open(&file, &bed.opened(), CAP).unwrap();
        let dir = copy.parent().unwrap().to_owned();
        // an edit saved beside it, had the folder let it
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(dir.join("a (edited).pages"), "the person's work").unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        age(&copy, OPENED_FOR + Duration::from_secs(60));
        sweep(&bed.opened(), SystemTime::now(), CAP);
        assert!(!copy.exists(), "its own file goes");
        assert_eq!(fs::read(dir.join("a (edited).pages")).unwrap(), b"the person's work");
        assert!(dir.is_dir(), "the folder stays with it");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_sweep_follows_no_link_and_goes_into_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let bed = Bed::new();
        let file = bed.open(&bed.file("a.pdf")).unwrap();
        let copy = copy_for_open(&file, &bed.opened(), CAP).unwrap();
        let dir = copy.parent().unwrap().to_owned();
        let precious = bed.outside("precious.pdf");
        let deeper = dir.join("deeper");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        // its file swapped for a link to a file elsewhere, and a folder beside it
        fs::remove_file(&copy).unwrap();
        symlink(&precious, &copy).unwrap();
        fs::create_dir(&deeper).unwrap();
        fs::write(deeper.join("x"), "x").unwrap();
        age(&copy, OPENED_FOR + Duration::from_secs(60));
        sweep(&bed.opened(), SystemTime::now(), CAP);
        assert_eq!(fs::read(&precious).unwrap(), b"elsewhere");
        assert!(fs::symlink_metadata(&copy).unwrap().file_type().is_symlink(), "not its file: left");
        assert!(deeper.join("x").exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_copy_on_a_miss_goes_folder_and_all() {
        let bed = Bed::new();
        let file = bed.open(&bed.file("a.pdf")).unwrap();
        let copy = copy_for_open(&file, &bed.opened(), CAP).unwrap();
        assert!(!open_or_discard(&copy, || false, |_| panic!("the open is gone")));
        assert!(!copy.parent().unwrap().exists());
        let copy = copy_for_open(&file, &bed.opened(), CAP).unwrap();
        assert!(!open_or_discard(&copy, || true, |_| false), "nothing opened it");
        assert!(!copy.parent().unwrap().exists());
        let copy = copy_for_open(&file, &bed.opened(), CAP).unwrap();
        assert!(open_or_discard(&copy, || true, |_| true));
        assert!(copy.exists());
        assert_eq!(fs::read_dir(bed.opened()).unwrap().count(), 2, "one copy and its record");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_viewer_reuses_its_copy_of_the_same_file_and_keeps_to_a_cap() {
        let bed = Bed::new();
        let path = bed.file("a.png");
        let file = bed.open(&path).unwrap();
        let first = copy_for_view(&file.file, "a.png", &bed.opened(), CAP).unwrap();
        let again = copy_for_view(&file.file, "a.png", &bed.opened(), CAP).unwrap();
        assert!(first.fresh && !again.fresh);
        assert_eq!(first.path, again.path, "one copy for one file");
        // the file changed: a new copy
        fs::write(&path, "changed, and longer").unwrap();
        let changed = bed.open(&path).unwrap();
        let third = copy_for_view(&changed.file, "a.png", &bed.opened(), CAP).unwrap();
        assert!(third.fresh && third.path != first.path);
        assert_eq!(fs::read(&third.path).unwrap(), b"changed, and longer");
        // past the cap, the oldest go, once no app is likely to hold them
        for i in 0..5 {
            let name = format!("p{i}.png");
            let f = bed.open(&bed.file(&name)).unwrap();
            copy_for_view(&f.file, &name, &bed.opened(), CAP).unwrap();
        }
        shift(&bed.opened(), IN_USE);
        cap(&bed.opened(), Cap { most: 3, bytes: u64::MAX }, None, None);
        assert_eq!(records(&bed.opened()).len(), 3);
        assert!(!first.path.exists());
        cap(&bed.opened(), Cap { most: 10, bytes: 1 }, None, None);
        assert_eq!(records(&bed.opened()).len(), 0, "past the bytes too");
    }

    #[cfg(target_os = "macos")]
    fn id_of(copy: &Path) -> String {
        copy.parent().unwrap().file_name().unwrap().to_str().unwrap().to_owned()
    }

    #[cfg(target_os = "macos")]
    fn copies_in(base: &Path) -> usize {
        records(base).len()
    }

    /// Makes every copy in `base` `by` older, keeping their order.
    #[cfg(target_os = "macos")]
    fn shift(base: &Path, by: Duration) {
        for (id, _, at) in records(base) {
            fs::File::open(base.join(format!("{id}.copy"))).unwrap().set_modified(at - by - Duration::from_secs(1)).unwrap();
        }
    }

    /// Preview has A open, moments old; B would push the folder past the
    /// cap. A stays, and B is refused, until A is past [`IN_USE`].
    #[cfg(target_os = "macos")]
    #[test]
    fn the_cap_never_takes_a_copy_an_app_may_have_open() {
        let bed = Bed::new();
        let cap = Cap { most: 64, bytes: 30 };
        let a = bed.open(&bed.file("a.pdf")).unwrap();
        let held = copy_for_open(&a, &bed.opened(), cap).unwrap();
        let path = bed.folder.join("open-1f/1/b.pdf");
        fs::write(&path, [b'x'; 28]).unwrap();
        let b = bed.open(&path).unwrap();
        assert_eq!(copy_for_open(&b, &bed.opened(), cap).unwrap_err().kind(), io::ErrorKind::FileTooLarge);
        assert!(held.exists(), "a copy an app may hold stays");
        assert_eq!(copies_in(&bed.opened()), 1, "and the new one was never made");
        shift(&bed.opened(), IN_USE);
        let made = copy_for_open(&b, &bed.opened(), cap).unwrap();
        assert!(made.exists() && !held.exists(), "past its while in use, it makes room");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_copy_cut_off_midway_counts_its_whole_size_against_the_cap() {
        let bed = Bed::new();
        let put = |out: fs::File, _: &Path| -> io::Result<fs::File> {
            let (_, record, _) = records(&bed.opened()).pop().unwrap();
            assert_eq!(record.bytes, 900, "the pending record's size is the file's");
            Ok(out)
        };
        copy_with("a.pdf", &bed.opened(), None, CAP, put, 900).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_record_naming_a_way_out_of_its_folder_is_no_record() {
        let bed = Bed::new();
        fs::create_dir_all(bed.opened()).unwrap();
        for name in ["../x.pdf", "a/b.pdf", "/etc/hosts", ".", "..", "a.pdf/"] {
            let at = bed.opened().join("r.copy");
            fs::write(&at, json!({"name": name, "ino": 1, "bytes": 1}).to_string()).unwrap();
            assert_eq!(Record::read(&at), None, "{name}");
        }
        let at = bed.opened().join("r.copy");
        fs::write(&at, json!({"name": "a.pdf", "ino": 1, "bytes": 1}).to_string()).unwrap();
        assert!(Record::read(&at).is_some());
    }

    /// The Bot killed midway through a copy, as a panic in the middle of its
    /// bytes leaves it: the folder and a marked file, nothing more.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_copy_cut_off_midway_is_still_the_sweeps() {
        let bed = Bed::new();
        let cut = std::panic::catch_unwind(|| {
            let put = |out: fs::File, _: &Path| -> io::Result<fs::File> {
                drop(out);
                panic!("killed midway")
            };
            let _ = copy_with("a.pdf", &bed.opened(), None, CAP, put, 5);
        });
        assert!(cut.is_err());
        let dirs: Vec<PathBuf> = fs::read_dir(bed.opened()).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        assert_eq!(dirs.len(), 1, "the cut copy's folder is there");
        let copy = dirs[0].join("a.pdf");
        assert!(copy.exists());
        age(&copy, OPENED_FOR + Duration::from_secs(60));
        sweep(&bed.opened(), SystemTime::now(), CAP);
        assert!(!dirs[0].exists(), "a day on, the sweep took it");
        assert_eq!(fs::read_dir(bed.opened()).unwrap().count(), 0, "its record too");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_copy_still_being_made_is_left_by_the_sweep_and_the_cap() {
        let bed = Bed::new();
        let put = |out: fs::File, to: &Path| {
            // the sweep, with no room at all, while this one copies
            sweep(&bed.opened(), SystemTime::now(), Cap { most: 0, bytes: 0 });
            assert!(to.exists(), "a copy being made is no one's to take");
            Ok(out)
        };
        let copy = copy_with("b.pdf", &bed.opened(), None, Cap { most: 1, bytes: u64::MAX }, put, 0).unwrap();
        assert!(copy.exists());
        assert_eq!(copies_in(&bed.opened()), 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_open_keeps_the_copies_to_the_cap() {
        let bed = Bed::new();
        let file = bed.open(&bed.file("a.pdf")).unwrap();
        let two = Cap { most: 2, bytes: u64::MAX };
        let first = copy_for_open(&file, &bed.opened(), two).unwrap();
        copy_for_open(&file, &bed.opened(), two).unwrap();
        shift(&bed.opened(), IN_USE);
        let last = copy_for_open(&file, &bed.opened(), two).unwrap();
        assert_eq!(copies_in(&bed.opened()), 2);
        assert!(!first.exists() && last.exists(), "the oldest went, the new one stays");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_file_over_the_cap_gets_no_copy_and_takes_none_away() {
        let bed = Bed::new();
        let small = bed.open(&bed.file("a.pdf")).unwrap();
        let cap = Cap { most: 64, bytes: 50 };
        let kept = copy_for_open(&small, &bed.opened(), cap).unwrap();
        let path = bed.folder.join("open-1f/1/big.pdf");
        fs::write(&path, [b'x'; 100]).unwrap();
        let big = bed.open(&path).unwrap();
        assert_eq!(copy_for_open(&big, &bed.opened(), cap).unwrap_err().kind(), io::ErrorKind::FileTooLarge);
        assert_eq!(copy_for_view(&big.file, "big.pdf", &bed.opened(), cap).unwrap_err().kind(), io::ErrorKind::FileTooLarge);
        assert!(kept.exists(), "no copy taken away for one that never fit");
        assert_eq!(fs::read_dir(bed.opened()).unwrap().count(), 2, "one copy and its record, no other");
        // one that grows past the cap while it copies stops there
        let grown = copy_with("big.pdf", &bed.opened(), None, cap, |out, to| fill(&big.file, out, to, false, Some(cap.bytes)), 10);
        assert_eq!(grown.unwrap_err().kind(), io::ErrorKind::FileTooLarge);
        assert_eq!(fs::read_dir(bed.opened()).unwrap().count(), 2);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_cap_never_takes_the_copy_it_returns() {
        let bed = Bed::new();
        let file = bed.open(&bed.file("a.pdf")).unwrap();
        let other = copy_for_open(&file, &bed.opened(), CAP).unwrap();
        let returned = copy_for_open(&file, &bed.opened(), CAP).unwrap();
        // the one returned reads as the oldest (a clock set back)
        age(&other, IN_USE + Duration::from_secs(60));
        age(&returned, IN_USE + Duration::from_secs(120));
        cap(&bed.opened(), Cap { most: 1, bytes: u64::MAX }, Some(&id_of(&returned)), None);
        assert!(returned.exists(), "kept");
        assert!(!other.exists());
        cap(&bed.opened(), Cap { most: 0, bytes: 0 }, Some(&id_of(&returned)), None);
        assert!(returned.exists(), "kept past any cap");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_sweep_keeps_the_copies_to_the_cap() {
        let bed = Bed::new();
        let file = bed.open(&bed.file("a.pdf")).unwrap();
        let first = copy_for_open(&file, &bed.opened(), CAP).unwrap();
        for _ in 0..2 {
            copy_for_open(&file, &bed.opened(), CAP).unwrap();
        }
        shift(&bed.opened(), IN_USE);
        sweep(&bed.opened(), SystemTime::now(), Cap { most: 2, bytes: u64::MAX });
        assert_eq!(copies_in(&bed.opened()), 2);
        assert!(!first.exists(), "the oldest went");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_pipe_in_a_records_place_never_holds_a_discard() {
        let bed = Bed::new();
        let file = bed.open(&bed.file("a.pdf")).unwrap();
        let copy = copy_for_open(&file, &bed.opened(), CAP).unwrap();
        let record = bed.opened().join(format!("{}.copy", id_of(&copy)));
        fs::remove_file(&record).unwrap();
        let c = std::ffi::CString::new(record.to_str().unwrap()).unwrap();
        // SAFETY: mkfifo with a NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        let at = copy.clone();
        std::thread::spawn(move || {
            discard(&at);
            let _ = tx.send(());
        });
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok(), "the discard came back");
        assert!(copy.exists(), "no record of it: left");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn marking_a_copy_used_or_unlocking_its_folder_follows_no_link() {
        use std::os::unix::fs::PermissionsExt;
        let bed = Bed::new();
        let target = bed.outside("target.pdf");
        let old = SystemTime::now() - Duration::from_secs(3600);
        fs::File::open(&target).unwrap().set_modified(old).unwrap();
        let link = bed.base.join("record.copy");
        symlink(&target, &link).unwrap();
        assert!(touch(&link).is_err());
        assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), old, "the file it points at is untouched");
        let dir = bed.base.join("elsewhere");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let dir_link = bed.base.join("copy-folder");
        symlink(&dir, &dir_link).unwrap();
        assert!(chmod_dir(&dir_link, 0o700).is_err());
        assert_eq!(mode(&dir), 0o755, "the folder it points at keeps its mode");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_open_asks_the_screen_in_the_step_that_opens() {
        use std::cell::Cell;
        let bed = Bed::new();
        let file = bed.open(&bed.file("a.pdf")).unwrap();
        let copy = copy_for_open(&file, &bed.opened(), CAP).unwrap();
        let asked = Cell::new(false);
        let on_top = || {
            asked.set(true);
            false
        };
        assert!(!open_or_discard(&copy, on_top, |_| panic!("the screen left the top")));
        assert!(asked.get());
        assert!(!copy.parent().unwrap().exists(), "the copy went");
        let copy = copy_for_open(&file, &bed.opened(), CAP).unwrap();
        let on_top = || {
            asked.set(false);
            true
        };
        assert!(open_or_discard(&copy, on_top, |_| !asked.get()), "asked before it opened");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_save_with_no_downloads_folder_writes_nothing() {
        let bed = Bed::new();
        let file = bed.save(&bed.file("a.png")).unwrap();
        assert!(save_in(&file, &bed.base.join("gone")).is_err());
        assert!(!bed.base.join("gone").exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_second_name_keeps_its_extension_and_a_bare_name_takes_the_number_at_the_end() {
        assert_eq!(numbered("IMG_4250.png", 1), "IMG_4250.png");
        assert_eq!(numbered("IMG_4250.png", 2), "IMG_4250 (2).png");
        assert_eq!(numbered("archive.tar.gz", 3), "archive.tar (3).gz");
        assert_eq!(numbered("README", 2), "README (2)");
        assert_eq!(numbered(".hidden", 2), ".hidden (2)");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_file_the_plugin_marked_already_marks_again_as_the_bots() {
        let bed = Bed::new();
        let path = bed.file("a.pdf");
        let plugin = "0083;00000000;Plugin;";
        let out = std::process::Command::new("/usr/bin/xattr").args(["-w", "com.apple.quarantine", plugin]).arg(&path).status().unwrap();
        assert!(out.success());
        assert!(mark_downloaded(&path));
        let mark = quarantine(&path).expect("marked");
        assert_ne!(mark, plugin, "the Bot's own mark, written over the plugin's");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_marked_file_reads_as_a_download() {
        let bed = Bed::new();
        let path = bed.outside("a.pdf");
        assert_eq!(quarantine(&path), None);
        assert!(!is_marked(&path));
        assert!(mark_downloaded(&path));
        assert!(quarantine(&path).is_some() && is_marked(&path));
    }
}
