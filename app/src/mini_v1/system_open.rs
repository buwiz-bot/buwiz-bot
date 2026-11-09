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
/// reaches the owner's.
#[cfg(target_os = "macos")]
pub fn opened_dir() -> PathBuf {
    let test = buwiz_protocol::env::var_os("BUWIZ_DATA_DIR");
    opened_dir_of(std::env::var_os("HOME").as_deref(), test.as_deref())
}

/// [`opened_dir`] for `home` and a test instance's data folder `test`.
#[cfg(target_os = "macos")]
fn opened_dir_of(home: Option<&std::ffi::OsStr>, test: Option<&std::ffi::OsStr>) -> PathBuf {
    match test.filter(|t| !t.is_empty()) {
        Some(dir) => Path::new(dir).join("Caches/opened"),
        None => Path::new(home.unwrap_or_default()).join("Library/Caches").join(crate::identity::APP_ID).join("opened"),
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
/// too.
#[cfg(target_os = "macos")]
fn fill(from: &fs::File, mut out: fs::File, to: &Path, sync: bool) -> io::Result<()> {
    use std::{io::Write, os::unix::fs::FileExt};
    let mut copy = || -> io::Result<()> {
        let mut buf = vec![0; 1 << 16];
        let mut at = 0;
        loop {
            let n = from.read_at(&mut buf, at)?;
            if n == 0 {
                return if sync { out.sync_all() } else { Ok(()) };
            }
            out.write_all(&buf[..n])?;
            at += n as u64;
        }
    };
    let copied = copy();
    if copied.is_err() {
        drop(out);
        let _ = fs::remove_file(to);
    }
    copied
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
        return fill(&file.file, out, &to, true).map(|()| to);
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
/// `base`, under its own name: the copy's path, for [`open_copy`].
#[cfg(target_os = "macos")]
pub fn copy_for_open(file: &LocalFile, base: &Path) -> io::Result<PathBuf> {
    copy_file(&file.file, &file.name, base)
}

/// Copies all of `from`, named `name` on disk, into a new folder of its own
/// in `base`, under that name, marked: the copy's path. The
/// viewers' pictures and PDFs are shown from such a copy too. A copy that
/// fails takes its folder with it.
#[cfg(target_os = "macos")]
pub fn copy_file(from: &fs::File, name: &str, base: &Path) -> io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    private_dir(base)?;
    let dir = base.join(super::keys::random_id());
    fs::DirBuilder::new().mode(0o700).create(&dir)?;
    let to = dir.join(name);
    let copied = create_marked(&to).and_then(|out| fill(from, out, &to, false));
    if let Err(e) = copied {
        let _ = fs::remove_dir_all(&dir);
        return Err(e);
    }
    Ok(to)
}

/// How a hand-off that copies went ([`copy_between_checks`]).
#[cfg(target_os = "macos")]
#[derive(Debug, PartialEq, Eq)]
pub enum Handed {
    /// The screen wasn't the person's any more before the copy: none made.
    LeftBefore,
    /// The copy failed, and took its folder.
    Failed(io::ErrorKind),
    /// The screen wasn't the person's any more once the copy was there: it
    /// went again.
    LeftAfter,
    /// Handed on to `done`.
    Done,
}

/// A hand-off that copies, in its order: `shows()` (the open still the
/// person's, on top), the copy, `shows()` again, then `done` with the copy. A
/// copy can take a while, and the person can leave meanwhile: a miss after it
/// takes the copy, and its folder, away again.
#[cfg(target_os = "macos")]
pub fn copy_between_checks(shows: &dyn Fn() -> bool, copy: impl FnOnce() -> io::Result<PathBuf>, done: impl FnOnce(PathBuf)) -> Handed {
    if !shows() {
        return Handed::LeftBefore;
    }
    let copy = match copy() {
        Ok(copy) => copy,
        Err(e) => return Handed::Failed(e.kind()),
    };
    if !shows() {
        let _ = fs::remove_dir_all(copy.parent().unwrap_or(&copy));
        return Handed::LeftAfter;
    }
    done(copy);
    Handed::Done
}

/// Takes away the copies in `base` older than [`OPENED_FOR`] at `now`.
#[cfg(target_os = "macos")]
pub fn sweep(base: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(base) else { return };
    for entry in entries.flatten() {
        let old = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age > OPENED_FOR));
        if old {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// How often the Bot sweeps [`opened_dir`] while it runs.
#[cfg(target_os = "macos")]
pub const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);

/// [`sweep`]s `base` now and every `every` after, for good: a thread's whole
/// work.
#[cfg(target_os = "macos")]
pub fn sweep_every(base: &Path, every: Duration) {
    loop {
        sweep(base, SystemTime::now());
        std::thread::sleep(every);
    }
}

/// [`sweep_every`] of [`opened_dir`], from launch, hourly: on a thread of its
/// own.
#[cfg(target_os = "macos")]
pub fn sweep_while_running() {
    sweep_every(&opened_dir(), SWEEP_EVERY);
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
            let _ = fs::remove_dir_all(&self.base);
        }
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
        assert_eq!(opened_dir_of(Some(home), None), PathBuf::from("/Users/someone/Library/Caches/com.buwiz.bot/opened"));
        assert_eq!(opened_dir_of(Some(home), Some(std::ffi::OsStr::new(""))), PathBuf::from("/Users/someone/Library/Caches/com.buwiz.bot/opened"));
        let test = std::ffi::OsStr::new("/tmp/bench/data");
        assert_eq!(opened_dir_of(Some(home), Some(test)), PathBuf::from("/tmp/bench/data/Caches/opened"), "a test instance's own");
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
            copy_for_open(&file, &bed.opened())
        };
        assert_eq!(copy_between_checks(&answers(&[false]), copy, |_| panic!("not handed")), Handed::LeftBefore);
        assert!(!made.get(), "no copy for a screen already left");
        let copy = || copy_for_open(&file, &bed.opened());
        assert_eq!(copy_between_checks(&answers(&[true, false]), copy, |_| panic!("not handed")), Handed::LeftAfter);
        assert_eq!(fs::read_dir(bed.opened()).unwrap().count(), 0, "the copy went again");
        let got = Cell::new(None);
        let copy = || copy_for_open(&file, &bed.opened());
        assert_eq!(copy_between_checks(&answers(&[true, true]), copy, |c| got.set(Some(c))), Handed::Done);
        assert_eq!(fs::read(got.take().unwrap()).unwrap(), b"a.pdf");
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
        let copy = copy_for_open(&open, &bed.opened()).unwrap();
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
        use std::os::unix::fs::PermissionsExt;
        let bed = Bed::new();
        let file = bed.open(&bed.file("Invoice.pdf")).unwrap();
        let copy = copy_for_open(&file, &bed.opened()).unwrap();
        assert_eq!(copy.file_name().unwrap(), "Invoice.pdf");
        assert!(quarantine(&copy).is_some() && is_marked(&copy));
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&bed.opened()), 0o700);
        assert_eq!(mode(copy.parent().unwrap()), 0o700);
        // a second open of the same name gets a folder of its own, and all of the file
        let again = copy_for_open(&file, &bed.opened()).unwrap();
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
        assert_eq!(fs::read(copy_for_open(&twin, &bed.opened()).unwrap()).unwrap(), b"Invoice.pdf");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_copy_that_fails_leaves_no_folder_behind() {
        let bed = Bed::new();
        let path = bed.file("a.pdf");
        // a file that can't be read from fails the copy after its folder is made
        let unreadable = fs::OpenOptions::new().write(true).open(&path).unwrap();
        assert!(copy_file(&unreadable, "a.pdf", &bed.opened()).is_err());
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
        let old = base.join("old");
        fs::create_dir_all(&old).unwrap();
        fs::File::open(&old).unwrap().set_modified(SystemTime::now() - OPENED_FOR - Duration::from_secs(60)).unwrap();
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
        let old = copy_for_open(&file, &bed.opened()).unwrap().parent().unwrap().to_owned();
        let fresh = copy_for_open(&file, &bed.opened()).unwrap().parent().unwrap().to_owned();
        let then = SystemTime::now() - OPENED_FOR - Duration::from_secs(60);
        fs::File::open(&old).unwrap().set_modified(then).unwrap();
        sweep(&bed.opened(), SystemTime::now());
        assert!(!old.exists(), "older than a day");
        assert!(fresh.exists());
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
