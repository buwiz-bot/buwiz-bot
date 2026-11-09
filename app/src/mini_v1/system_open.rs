//! `_meta["buwiz/open_with"] = {path}` and `_meta["buwiz/save"] = {path,
//! name}`: a file a first-party plugin asks the Bot to open in the system's
//! default app, or to save to the person's Downloads folder. Both pass
//! `document`'s first checks ([`document::checked`]): first-party, the
//! person's surface and not the AI's, a real file in that plugin's own folder
//! with no link on the way. Only the field's `path` is read: the file's name
//! and kind are the ones on disk.
//!
//! An open also needs a kind on [`SAFE`], by the extension on disk: anything
//! else opens nothing, whatever app the system has for it. A save takes any
//! kind, as a browser's download does.
//!
//! Either marks its file as downloaded first (`com.apple.quarantine`, as a
//! browser does), so Gatekeeper and the app that opens it treat it as a file
//! from the internet: a mail's attachment is a stranger's. The plugin marks
//! what it writes too, but the Bot doesn't count on it: an open marks again,
//! and a save marks its copy, which a copy's xattrs may not reach.
//!
//! A refusal carries no part of the path, as `document`'s does ([`Refused`]).
//! The check looks once, as `document`'s does: a first-party plugin that swaps
//! its own file after the check gets the swapped one opened or saved.

use std::path::Path;
#[cfg(target_os = "macos")]
use std::{fs, io, path::PathBuf};

use serde_json::Value;

use super::{
    attribution::Actor,
    document::{self, Refused},
};

/// The kinds `buwiz/open_with` opens, by the extension on disk: pictures,
/// PDFs, text, Office and iWork files, calendar invites, archives. No
/// program, script, disk image, installer or web page, and no legacy Office
/// file (`doc`, `xls`, `ppt`), which can carry macros.
pub const SAFE: [&str; 17] = ["png", "jpg", "jpeg", "gif", "webp", "heic", "pdf", "txt", "csv", "docx", "xlsx", "pptx", "pages", "numbers", "key", "ics", "zip"];

/// A file that passed [`check_open`] or [`check_save`]: its real path, and
/// the name it has there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile {
    path: String,
    name: String,
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
    let (path, name) = document::checked(first_party, actor, folder, field)?;
    if !SAFE.contains(&document::ext(&name).as_str()) {
        return Err(Refused::Kind);
    }
    Ok(LocalFile { path, name })
}

/// Whether `buwiz/save`'s `field` may save: [`check_open`] without the kind.
pub fn check_save(first_party: bool, actor: Actor, folder: Option<&Path>, field: &Value) -> Result<LocalFile, Refused> {
    let (path, name) = document::checked(first_party, actor, folder, field)?;
    Ok(LocalFile { path, name })
}

/// `<home>/Downloads`: `None` without a real home.
#[cfg(target_os = "macos")]
pub fn downloads() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    home.is_absolute().then(|| home.join("Downloads"))
}

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

/// Copies `file` into `dir` under its own name, or the first numbered one
/// free there, and marks the copy as downloaded: the path written. Nothing
/// there is ever written over, a link included (`create_new` won't follow
/// one), and a copy that fails or can't be marked is taken away again.
#[cfg(target_os = "macos")]
pub fn save_in(file: &LocalFile, dir: &Path) -> io::Result<PathBuf> {
    let mut from = fs::File::open(&file.path)?;
    for n in 1..=999 {
        let to = dir.join(numbered(&file.name, n));
        let mut out = match fs::OpenOptions::new().write(true).create_new(true).open(&to) {
            Ok(out) => out,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        let copied = io::copy(&mut from, &mut out).and_then(|_| out.sync_all());
        drop(out);
        let marked = copied.and_then(|()| if mark_downloaded(&to) { Ok(()) } else { Err(io::Error::other("not marked")) });
        return match marked {
            Ok(()) => Ok(to),
            Err(e) => {
                let _ = fs::remove_file(&to);
                Err(e)
            }
        };
    }
    Err(io::Error::from(io::ErrorKind::AlreadyExists))
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

/// Marks `file` as downloaded, then opens it in the system's default app for
/// its kind, through NSWorkspace as `native::open_in_browser` opens a URL. A
/// file that can't be marked opens nothing. On the main thread.
#[cfg(target_os = "macos")]
pub fn open(file: &LocalFile) -> bool {
    use nsh_platform_macos::{
        objc2::{msg_send, rc::Retained, runtime::{AnyClass, AnyObject}},
        objc2_foundation::{NSString, NSURL},
    };
    if !mark_downloaded(Path::new(&file.path)) {
        return false;
    }
    let Some(class) = AnyClass::get(c"NSWorkspace") else { return false };
    let url = NSURL::fileURLWithPath(&NSString::from_str(&file.path));
    // SAFETY: NSWorkspace's documented methods, on the main thread.
    unsafe {
        let ws: Retained<AnyObject> = msg_send![class, sharedWorkspace];
        msg_send![&*ws, openURL: &*url]
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink, path::PathBuf};

    use serde_json::json;

    use super::*;

    /// A plugin's folder with `open/1/` in it, a folder beside it, and a
    /// Downloads folder.
    struct Bed {
        base: PathBuf,
        folder: PathBuf,
    }

    impl Bed {
        fn new() -> Bed {
            let base = crate::mini_v1::storage::scratch_root("system-open");
            let folder = base.join("app.buwiz.fixture");
            fs::create_dir_all(folder.join("open/1")).unwrap();
            fs::create_dir_all(base.join("elsewhere")).unwrap();
            fs::create_dir_all(base.join("Downloads")).unwrap();
            Bed { base, folder }
        }

        fn file(&self, name: &str) -> PathBuf {
            let path = self.folder.join("open/1").join(name);
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
            "run.exe", "a.app", "a.command", "a.sh", "a.html", "a.htm", "a.svg", "a.dmg", "a.pkg", "a.jar", "a.scpt", "a.terminal", "a.webloc", "a.png.exe", "a.zip.command",
            "a.doc", "a.xls", "a.ppt", "a.docm", "a.xlsm", "noextension", ".png", "a.png ", "a.",
        ];
        for name in off {
            assert_eq!(bed.open(&bed.file(name)), Err(Refused::Kind), "{name}");
        }
    }

    #[test]
    fn a_png_name_cant_launder_an_exe() {
        let bed = Bed::new();
        let field = json!({"path": bed.file("run.exe"), "name": "IMG_4250.png", "mime": "image/png"});
        assert_eq!(check_open(true, Actor::Person, Some(&bed.folder), &field), Err(Refused::Kind));
        let saved = check_save(true, Actor::Person, Some(&bed.folder), &field).unwrap();
        assert_eq!(saved.name(), "run.exe", "a save keeps the name on disk, not the field's");
    }

    #[test]
    fn the_ais_call_or_a_third_party_plugin_opens_and_saves_nothing() {
        let bed = Bed::new();
        let field = json!({"path": bed.file("a.png")});
        for check in [check_open, check_save] {
            assert_eq!(check(false, Actor::Person, Some(&bed.folder), &field), Err(Refused::ThirdParty));
            assert_eq!(check(true, Actor::Agent, Some(&bed.folder), &field), Err(Refused::NotThePerson));
            assert_eq!(check(true, Actor::Person, None, &field), Err(Refused::NoFolder));
            assert_eq!(check(true, Actor::Person, Some(&bed.folder), &json!({"name": "a.png"})), Err(Refused::BadField));
        }
    }

    #[test]
    fn a_link_or_a_file_outside_the_folder_opens_and_saves_nothing() {
        let bed = Bed::new();
        let outside = bed.outside("a.png");
        let link = bed.folder.join("open/1/link.png");
        symlink(&outside, &link).unwrap();
        symlink(bed.base.join("elsewhere"), bed.folder.join("open/2")).unwrap();
        let hard = bed.folder.join("open/1/hard.png");
        fs::hard_link(bed.outside("b.png"), &hard).unwrap();
        for (path, why) in [
            (outside.clone(), Refused::Outside),
            (bed.folder.join("../elsewhere/a.png"), Refused::Outside),
            (link, Refused::Link),
            (bed.folder.join("open/2/a.png"), Refused::Link),
            (hard, Refused::Link),
            (bed.folder.join("open/1/gone.png"), Refused::Missing),
            (bed.folder.join("open/1"), Refused::NotAFile),
        ] {
            assert_eq!(bed.open(&path), Err(why), "{}", path.display());
            assert_eq!(bed.save(&path), Err(why), "{}", path.display());
        }
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
        let file = bed.save(&bed.file("IMG_4250.png")).unwrap();
        let to = save_in(&file, &bed.downloads()).unwrap();
        assert_eq!(to, bed.downloads().join("IMG_4250.png"));
        assert_eq!(fs::read(&to).unwrap(), b"IMG_4250.png");
        assert!(quarantine(&to).is_some(), "marked");
        assert_eq!(quarantine(Path::new(&file.path)), None, "the plugin's own file is left as it was");
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
        assert_eq!(save_in(&file, &bed.downloads()).unwrap(), bed.downloads().join("IMG_4250 (4).png"));
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
    fn a_save_with_no_downloads_folder_writes_nothing() {
        let bed = Bed::new();
        let file = bed.save(&bed.file("a.png")).unwrap();
        assert!(save_in(&file, &bed.base.join("gone")).is_err());
        assert!(!bed.base.join("gone").exists());
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
        assert!(mark_downloaded(&path));
        assert!(quarantine(&path).is_some());
    }
}
