//! `_meta["buwiz/document"] = {path, name, mime}`: a file a first-party
//! plugin hands to the Bot's own viewers: Word or Spreadsheet, the lightbox
//! for a picture, the PDF view. No permission key grants this and the
//! plugin's word proves nothing, so [`check`] decides from what the Bot knows
//! and the disk says. Its checks go in the contract's order: first-party, the
//! person's surface and not the AI's, a real file in one of that plugin's
//! hand-off folders (`open-<hex>/<folder>/<file>` in its own folder, never its
//! state or caches; a Word or Sheet file also at 1.4.0's `open/<n>/<file>`),
//! a kind a viewer takes. What passes opens by the path found on disk, under
//! the name found there, and goes to its viewer by the extension there: the
//! field's `name` and `mime` are never read, so they can't launder a kind.
//!
//! A refusal opens nothing and carries no part of the path: [`Refused`] has
//! no data, so a log line can't hold one.
//!
//! **The second check is not proof of a press.** The actor says the AI hasn't
//! acted on this surface, so a first-party screen's own code at mount or on a
//! timer passes it too. Nothing here can tell those from a press: only the
//! engine knows it's inside a press, and it doesn't say.
//!
//! A picture or a PDF is never shown from the plugin's path: the Bot copies
//! the file it checked, from the file it opened at the check, into a marked
//! copy of its own ([`Document::needs_copy`], `system_open::copy_file`), and
//! the viewer shows that. Word and Sheet edit the file in place and save back
//! to its path, so they keep the path: [`Document::still_checked`] looks
//! again, on the engine thread, just before the viewer opens it.
//!
//! **What this can't do.** For Word and Sheet, the checks hold until the
//! viewer opens the file, and no longer. The viewer goes by the path
//! for as long as it is open: it reads the file when it starts and at every
//! reload, and it reloads by itself when the file changes, and every save
//! finds the path again. So there is no moment when the checks hold for good.
//! A plugin that swaps the file, then or any time after, gets the swapped file
//! shown, and one that swaps a folder on the way for a link gets a save written
//! where the link points, outside its folder. Nothing checks again, and that is
//! accepted: it would have to be a first-party plugin, working against its own
//! folder.

use std::{
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
};

use serde_json::Value;

use super::{
    attribution::Actor,
    consent::{self, Ask},
    perms,
};
use crate::store::Attach;

/// Why a document opens nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The open's screen ended before the result came.
    Closed,
    /// Any plugin but an approved first-party one.
    ThirdParty,
    /// The call was not the person's: the AI's, or a routine's.
    NotThePerson,
    /// No `path` string in the field.
    BadField,
    /// The host knows no folder for the plugin.
    NoFolder,
    /// The path is not inside the plugin's own folder.
    Outside,
    /// A symlink or a second hard link on the way.
    Link,
    /// Inside the folder, but not at `open-<hex>/<folder>/<file>`.
    Shape,
    /// Nothing there.
    Missing,
    /// There, but no regular file.
    NotAFile,
    /// A kind the viewer doesn't take.
    Kind,
    /// No press of the person's in the open just before the call, or a
    /// hand-off spent it (`attribution`'s `fresh_press`).
    NoPress,
}

/// A file that passed [`check`]: its real path, the name it has there, the
/// file as opened at the check, and where it was checked.
#[derive(Debug, Clone)]
pub struct Document {
    path: String,
    name: String,
    file: std::sync::Arc<fs::File>,
    folder: PathBuf,
    /// The checked file's device and inode.
    id: (u64, u64),
    /// The Bot's own copy the viewer shows instead, once it is made.
    copy: Option<String>,
}

impl PartialEq for Document {
    fn eq(&self, other: &Document) -> bool {
        (&self.path, &self.name, self.id, &self.copy) == (&other.path, &other.name, other.id, &other.copy)
    }
}

/// The pictures the lightbox shows.
const IMAGES: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "heic"];

/// The extension of the file name `name`, lowercased: none for `.docx` or
/// `noextension`.
pub(super) fn ext(name: &str) -> String {
    // ASCII only: Unicode folds the Kelvin sign to `k`, and `.\u{212A}ey` is no `.key`
    Path::new(name).extension().and_then(OsStr::to_str).map(str::to_ascii_lowercase).unwrap_or_default()
}

/// The viewer kind `store.open_file` routes by, from the name on disk:
/// `image` to the lightbox, `pdf` to the PDF view, `file` for the rest, which
/// the viewer routes by the extension.
fn kind_of(name: &str) -> &'static str {
    match ext(name).as_str() {
        e if IMAGES.contains(&e) => "image",
        "pdf" => "pdf",
        _ => "file",
    }
}

impl Document {
    /// What `store.open_file` takes, its kind from the name on disk only, at
    /// the Bot's copy once there is one.
    pub fn attach(&self) -> Attach {
        Attach {
            kind: kind_of(&self.name).to_owned(),
            path: self.copy.clone().unwrap_or_else(|| self.path.clone()),
            name: self.name.clone(),
            ext: crate::files::ext_of(&self.name, ""),
            ..Default::default()
        }
    }

    /// Its name on disk.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The file, as opened at the check.
    pub fn file(&self) -> &fs::File {
        &self.file
    }

    /// Whether the viewer shows a copy of the Bot's, not the plugin's path:
    /// a picture or a PDF, which no viewer of the Bot's writes back.
    pub fn needs_copy(&self) -> bool {
        self.copy.is_none() && kind_of(&self.name) != "file"
    }

    /// This, shown from the Bot's own copy at `copy`.
    pub fn with_copy(mut self, copy: &Path) -> Document {
        self.copy = copy.to_str().map(str::to_owned);
        self
    }

    /// Whether what the viewer is about to open is still what was checked: a
    /// copy of the Bot's is; the plugin's path is if it passes the checks
    /// again and is the same file.
    pub fn still_checked(&self) -> bool {
        use std::os::unix::fs::MetadataExt;
        if self.copy.is_some() {
            return true;
        }
        let again = file_in(&self.folder, Path::new(&self.path), true).ok().and_then(|(real, f)| Some((real, f.metadata().ok()?)));
        again.is_some_and(|(real, m)| real == Path::new(&self.path) && (m.dev(), m.ino()) == self.id)
    }
}

/// `<home>/Library/Application Support/<plugin>`: where plan § 6 has a
/// plugin keep its files. `None` without a real home or plugin id.
pub fn folder_of(home: Option<&OsStr>, plugin: &str) -> Option<PathBuf> {
    let home = Path::new(home?);
    (home.is_absolute() && perms::plugin_id_ok(plugin)).then(|| home.join("Library/Application Support").join(plugin))
}

/// Whether `field`, from a call `actor` made to a plugin with data folder
/// `folder`, may open. `first_party` is the plugin's listing, read as
/// `FirstPartyAnswerCard` reads it.
pub fn check(first_party: bool, actor: Actor, folder: Option<&Path>, field: &Value) -> Result<Document, Refused> {
    use std::os::unix::fs::MetadataExt;
    let (path, name, file) = checked_in(first_party, actor, folder, field, true)?;
    if !crate::office::native_ext(&path) && kind_of(&name) == "file" {
        return Err(Refused::Kind);
    }
    let m = file.metadata().map_err(|_| Refused::Missing)?;
    let folder = folder.ok_or(Refused::NoFolder)?.to_owned();
    Ok(Document { path, name, file: std::sync::Arc::new(file), folder, id: (m.dev(), m.ino()), copy: None })
}

/// [`check`]'s first three checks, which `buwiz/open_with` and `buwiz/save`
/// share (`system_open`): the real path of the file `field` names, its name
/// there, both UTF-8, and the file itself, opened as it was checked.
pub(super) fn checked(first_party: bool, actor: Actor, folder: Option<&Path>, field: &Value) -> Result<(String, String, fs::File), Refused> {
    checked_in(first_party, actor, folder, field, false)
}

/// [`checked`], taking Word and Sheet files at 1.4.0's `open/<n>/<file>`
/// too when `legacy`.
fn checked_in(first_party: bool, actor: Actor, folder: Option<&Path>, field: &Value, legacy: bool) -> Result<(String, String, fs::File), Refused> {
    if consent::ask(consent::inline_card(first_party)) != Ask::Nothing {
        return Err(Refused::ThirdParty);
    }
    if actor != Actor::Person {
        return Err(Refused::NotThePerson);
    }
    let claimed = field.get("path").and_then(Value::as_str).ok_or(Refused::BadField)?;
    let (real, file) = file_in(folder.ok_or(Refused::NoFolder)?, Path::new(claimed), legacy)?;
    let path = real.to_str().ok_or(Refused::Kind)?.to_owned();
    let name = real.file_name().and_then(OsStr::to_str).ok_or(Refused::Kind)?.to_owned();
    Ok((path, name, file))
}

/// Whether `rest`, below a plugin's folder, is `open-<hex>/<folder>/<file>`:
/// where a plugin hands its files over, and the only place the Bot takes one.
/// With `legacy`, a Word or Sheet file at `open/<n>/<file>` passes too:
/// where plugin 1.4.0 hands them over, and it runs beside this Bot until
/// 1.5.0 reaches the person.
fn handed_over(rest: &Path, legacy: bool) -> bool {
    let parts: Vec<Component> = rest.components().collect();
    let hex = |s: &OsStr| s.to_str().and_then(|s| s.strip_prefix("open-")).is_some_and(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()));
    let old = |s: &OsStr, file: &OsStr| legacy && s == "open" && file.to_str().is_some_and(crate::office::native_ext);
    matches!(parts.as_slice(), [Component::Normal(first), Component::Normal(_), Component::Normal(file)] if hex(first) || old(first, file))
}

/// `claimed` as the real path of a regular file at a hand-off place inside
/// `folder` ([`handed_over`]), with no symlink from the folder down, and that
/// file opened. Canonicalizing alone would follow a link that stays inside,
/// so each step is looked at unfollowed. What is above the folder is the
/// account's own: only that part may be a link (`/var` is one), and `claimed`
/// may spell it either way.
///
/// The file is opened without following a link and must be the very file the
/// walk found (its device and inode): a step swapped for a link after the walk
/// opens something else, and is refused. What reads from the open file, not
/// the path, reads what was checked.
fn file_in(folder: &Path, claimed: &Path, legacy: bool) -> Result<(PathBuf, fs::File), Refused> {
    let linked = fs::symlink_metadata(folder).map_err(|_| Refused::NoFolder)?.file_type().is_symlink();
    if linked {
        return Err(Refused::Link);
    }
    let root = fs::canonicalize(folder).map_err(|_| Refused::NoFolder)?;
    let rest = claimed.strip_prefix(folder).or_else(|_| claimed.strip_prefix(&root)).map_err(|_| Refused::Outside)?;
    let mut at = root.clone();
    let mut last = None;
    for part in rest.components() {
        let Component::Normal(part) = part else { return Err(Refused::Outside) };
        at.push(part);
        let meta = fs::symlink_metadata(&at).map_err(|_| Refused::Missing)?;
        if meta.file_type().is_symlink() {
            return Err(Refused::Link);
        }
        last = Some(meta);
    }
    let meta = last.ok_or(Refused::NotAFile)?;
    if !meta.file_type().is_file() {
        return Err(Refused::NotAFile);
    }
    if !handed_over(rest, legacy) {
        return Err(Refused::Shape);
    }
    // this stops the viewer showing a file from elsewhere under a second name,
    // as the symlink rule does. Reading is all it reaches: a save renames over
    // this name, so the other name keeps the old file
    if std::os::unix::fs::MetadataExt::nlink(&meta) != 1 {
        return Err(Refused::Link);
    }
    let real = fs::canonicalize(&at).map_err(|_| Refused::Missing)?;
    if real != at || !real.starts_with(&root) {
        return Err(Refused::Outside);
    }
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    // non-blocking: a FIFO swapped in can't hang the open
    let file = fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(&real).map_err(|_| Refused::Link)?;
    let opened = file.metadata().map_err(|_| Refused::Missing)?;
    if !opened.file_type().is_file() || opened.nlink() != 1 || (opened.dev(), opened.ino()) != (meta.dev(), meta.ino()) {
        return Err(Refused::Link);
    }
    Ok((real, file))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use serde_json::json;

    use super::*;

    /// A plugin's folder with `open-1f/1/` in it, and a folder beside it.
    struct Bed {
        base: PathBuf,
        folder: PathBuf,
    }

    impl Bed {
        fn new() -> Bed {
            let base = crate::mini_v1::storage::scratch_root("document");
            let folder = base.join("app.buwiz.fixture");
            fs::create_dir_all(folder.join("open-1f/1")).unwrap();
            fs::create_dir_all(base.join("elsewhere")).unwrap();
            Bed { base, folder }
        }

        /// Writes `open-1f/1/<name>`: its path as the plugin would send it.
        fn file(&self, name: &str) -> PathBuf {
            let path = self.folder.join("open-1f/1").join(name);
            fs::write(&path, "x").unwrap();
            path
        }

        /// Writes `elsewhere/<name>`.
        fn outside(&self, name: &str) -> PathBuf {
            let path = self.base.join("elsewhere").join(name);
            fs::write(&path, "x").unwrap();
            path
        }

        fn ask(&self, path: &Path) -> Result<Document, Refused> {
            check(true, Actor::Person, Some(&self.folder), &json!({"path": path, "name": "x.docx", "mime": "x"}))
        }
    }

    impl Drop for Bed {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    #[test]
    fn a_good_docx_and_xlsx_open_by_their_real_path() {
        let bed = Bed::new();
        for name in ["Report.docx", "Book.xlsx", "LOUD.DOCX"] {
            let doc = bed.ask(&bed.file(name)).unwrap();
            let real = fs::canonicalize(bed.folder.join("open-1f/1").join(name)).unwrap();
            assert_eq!(doc.path, real.to_str().unwrap());
            let a = doc.attach();
            assert_eq!((a.kind.as_str(), a.name.as_str(), a.path.as_str()), ("file", name, doc.path.as_str()));
        }
    }

    #[test]
    fn a_path_may_spell_the_account_side_of_the_folder_either_way() {
        let bed = Bed::new();
        bed.file("a.docx");
        // a link above the folder is the account's own
        let above = bed.base.join("above");
        symlink(&bed.base, &above).unwrap();
        let folder = above.join("app.buwiz.fixture");
        let literal = folder.join("open-1f/1/a.docx");
        let real = fs::canonicalize(&literal).unwrap();
        for path in [&literal, &real] {
            let doc = check(true, Actor::Person, Some(&folder), &json!({"path": path})).unwrap();
            assert_eq!(doc.path, real.to_str().unwrap());
        }
    }

    #[test]
    fn a_third_party_plugins_field_is_ignored_before_the_disk_is_read() {
        let bed = Bed::new();
        let field = json!({"path": bed.file("a.docx")});
        assert_eq!(check(false, Actor::Person, Some(&bed.folder), &field), Err(Refused::ThirdParty));
        assert_eq!(check(false, Actor::Agent, None, &json!(1)), Err(Refused::ThirdParty), "the first check goes first");
    }

    #[test]
    fn a_call_of_the_ai_or_a_routine_opens_nothing() {
        let bed = Bed::new();
        let field = json!({"path": bed.file("a.docx")});
        assert_eq!(check(true, Actor::Agent, Some(&bed.folder), &field), Err(Refused::NotThePerson));
        assert_eq!(check(true, Actor::Agent, None, &json!(1)), Err(Refused::NotThePerson), "before the field is read");
    }

    #[test]
    fn a_field_with_no_path_string_opens_nothing() {
        let bed = Bed::new();
        for field in [json!(null), json!("a.docx"), json!({}), json!({"path": 1}), json!({"name": "a.docx"})] {
            assert_eq!(check(true, Actor::Person, Some(&bed.folder), &field), Err(Refused::BadField), "{field}");
        }
    }

    #[test]
    fn a_host_with_no_folder_opens_nothing() {
        let bed = Bed::new();
        let field = json!({"path": bed.file("a.docx")});
        assert_eq!(check(true, Actor::Person, None, &field), Err(Refused::NoFolder));
        assert_eq!(check(true, Actor::Person, Some(&bed.base.join("gone")), &field), Err(Refused::NoFolder));
    }

    #[test]
    fn a_path_outside_the_folder_opens_nothing() {
        let bed = Bed::new();
        assert_eq!(bed.ask(&bed.outside("a.docx")), Err(Refused::Outside));
        assert_eq!(bed.ask(Path::new("open-1f/1/a.docx")), Err(Refused::Outside), "relative");
        // a folder whose name only starts the same
        let twin = bed.base.join("app.buwiz.fixture-two");
        fs::create_dir_all(&twin).unwrap();
        fs::write(twin.join("a.docx"), "x").unwrap();
        assert_eq!(bed.ask(&twin.join("a.docx")), Err(Refused::Outside));
    }

    #[test]
    fn a_path_that_climbs_with_dots_opens_nothing() {
        let bed = Bed::new();
        bed.outside("a.docx");
        bed.file("b.docx");
        assert_eq!(bed.ask(&bed.folder.join("open-1f/../../elsewhere/a.docx")), Err(Refused::Outside));
        assert_eq!(bed.ask(&bed.folder.join("../elsewhere/a.docx")), Err(Refused::Outside));
        // it stays inside, and still isn't what the plugin was told to send
        assert_eq!(bed.ask(&bed.folder.join("open-1f/1/../1/b.docx")), Err(Refused::Outside));
    }

    #[test]
    fn a_symlinked_file_opens_nothing_wherever_it_points() {
        let bed = Bed::new();
        let link = bed.folder.join("open-1f/1/out.docx");
        symlink(bed.outside("a.docx"), &link).unwrap();
        assert_eq!(bed.ask(&link), Err(Refused::Link));
        let inner = bed.folder.join("open-1f/1/in.docx");
        symlink(bed.file("b.docx"), &inner).unwrap();
        assert_eq!(bed.ask(&inner), Err(Refused::Link), "a link that stays inside is still a link");
    }

    #[test]
    fn a_symlinked_folder_on_the_way_opens_nothing_wherever_it_points() {
        let bed = Bed::new();
        bed.outside("a.docx");
        symlink(bed.base.join("elsewhere"), bed.folder.join("open-1f/2")).unwrap();
        assert_eq!(bed.ask(&bed.folder.join("open-1f/2/a.docx")), Err(Refused::Link));
        bed.file("b.docx");
        symlink(bed.folder.join("open-1f/1"), bed.folder.join("open-1f/3")).unwrap();
        assert_eq!(bed.ask(&bed.folder.join("open-1f/3/b.docx")), Err(Refused::Link));
    }

    #[test]
    fn a_plugin_folder_that_is_itself_a_link_opens_nothing() {
        let bed = Bed::new();
        let path = bed.outside("a.docx");
        let link = bed.base.join("app.buwiz.linked");
        symlink(bed.base.join("elsewhere"), &link).unwrap();
        let field = json!({"path": link.join("a.docx")});
        assert_eq!(check(true, Actor::Person, Some(&link), &field), Err(Refused::Link));
        assert_eq!(check(true, Actor::Person, Some(&link), &json!({"path": path})), Err(Refused::Link));
    }

    #[test]
    fn a_hard_link_to_a_file_elsewhere_opens_nothing() {
        let bed = Bed::new();
        let link = bed.folder.join("open-1f/1/a.docx");
        fs::hard_link(bed.outside("a.docx"), &link).unwrap();
        assert_eq!(bed.ask(&link), Err(Refused::Link));
    }

    /// What the comment in [`file_in`] says of a save.
    #[test]
    fn a_save_replaces_a_hard_linked_name_and_leaves_the_other_alone() {
        let bed = Bed::new();
        let other = bed.outside("a.docx");
        let link = bed.folder.join("open-1f/1/a.docx");
        fs::hard_link(&other, &link).unwrap();
        crate::office::session::write_atomic(&link, b"saved").unwrap();
        assert_eq!(fs::read(&link).unwrap(), b"saved");
        assert_eq!(fs::read(&other).unwrap(), b"x");
    }

    #[test]
    fn a_directory_or_a_missing_file_opens_nothing() {
        let bed = Bed::new();
        let dir = bed.folder.join("open-1f/1/dir.docx");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(bed.ask(&dir), Err(Refused::NotAFile));
        assert_eq!(bed.ask(&bed.folder), Err(Refused::NotAFile), "the folder itself");
        assert_eq!(bed.ask(&bed.folder.join("open-1f/1/gone.docx")), Err(Refused::Missing));
        // a file where a folder should be
        let file = bed.file("a.docx");
        assert_eq!(bed.ask(&file.join("b.docx")), Err(Refused::Missing));
    }

    #[test]
    fn a_kind_the_viewer_does_not_take_opens_nothing() {
        let bed = Bed::new();
        for name in ["run.exe", "a.docx.exe", "a.pptx", "a.unknown", "noextension", ".docx", "a.png.exe", "a.svg", "a.heif", ".png", "a.png "] {
            assert_eq!(bed.ask(&bed.file(name)), Err(Refused::Kind), "{name}");
        }
    }

    #[test]
    fn only_a_file_in_a_hand_off_folder_opens() {
        let bed = Bed::new();
        let at = |rel: &str| {
            let path = bed.folder.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "x").unwrap();
            path
        };
        for rel in [
            "a.docx",
            "store/a.docx",
            "cache/1/a.docx",
            "open/a.docx",
            "open/1/2/a.docx",
            "open-/1/a.docx",
            "open-xyz/1/a.docx",
            "open-1f/a.docx",
            "open-1f/1/2/a.docx",
            "Open-1f/1/a.docx",
        ] {
            assert_eq!(bed.ask(&at(rel)), Err(Refused::Shape), "{rel}");
        }
        for rel in ["open-1f/1/a.docx", "open-0123456789abcdefABCDEF/x/a.docx"] {
            assert!(bed.ask(&at(rel)).is_ok(), "{rel}");
        }
    }

    #[test]
    fn a_word_or_sheet_file_also_opens_where_plugin_1_4_0_hands_it_over() {
        let bed = Bed::new();
        fs::create_dir_all(bed.folder.join("open/3")).unwrap();
        let at = |name: &str| {
            let path = bed.folder.join("open/3").join(name);
            fs::write(&path, "x").unwrap();
            path
        };
        for name in ["a.docx", "b.xlsx", "c.csv"] {
            assert!(bed.ask(&at(name)).is_ok(), "{name}");
        }
        for name in ["a.png", "b.pdf"] {
            assert_eq!(bed.ask(&at(name)), Err(Refused::Shape), "{name}: only the new place");
        }
        // and nothing but the viewer takes the old place
        let field = json!({"path": at("d.docx")});
        assert_eq!(checked(true, Actor::Person, Some(&bed.folder), &field).err(), Some(Refused::Shape));
    }

    #[test]
    fn a_word_file_swapped_after_the_check_doesnt_open() {
        let bed = Bed::new();
        let path = bed.file("a.docx");
        let doc = bed.ask(&path).unwrap();
        assert!(doc.still_checked());
        fs::remove_file(&path).unwrap();
        symlink(bed.outside("secret.docx"), &path).unwrap();
        assert!(!doc.still_checked(), "a link now");
        fs::remove_file(&path).unwrap();
        fs::write(&path, "another").unwrap();
        assert!(!doc.still_checked(), "another file under the name");
        assert!(!doc.needs_copy(), "Word saves back to the path");
    }

    #[test]
    fn a_picture_or_pdf_is_shown_from_a_copy() {
        let bed = Bed::new();
        for name in ["a.png", "b.pdf"] {
            let doc = bed.ask(&bed.file(name)).unwrap();
            assert!(doc.needs_copy(), "{name}");
            let copy = bed.base.join("copy").join(name);
            let shown = doc.with_copy(&copy);
            assert!(!shown.needs_copy() && shown.still_checked());
            assert_eq!(shown.attach().path, copy.to_str().unwrap());
        }
    }

    #[test]
    fn a_kelvin_sign_cant_pass_for_a_k() {
        assert_eq!(ext("x.\u{212A}EY"), "\u{212A}ey");
        assert_eq!(ext("x.JPEG"), "jpeg");
        let bed = Bed::new();
        assert_eq!(bed.ask(&bed.file("a.\u{212A}EY")), Err(Refused::Kind));
    }

    #[test]
    fn a_picture_goes_to_the_lightbox_and_a_pdf_to_the_pdf_view() {
        let bed = Bed::new();
        let cases = [
            ("IMG_4250.png", "image"),
            ("a.jpg", "image"),
            ("a.JPEG", "image"),
            ("a.gif", "image"),
            ("a.webp", "image"),
            ("IMG_1.HEIC", "image"),
            ("Invoice.pdf", "pdf"),
            ("Report.docx", "file"),
            ("Book.xlsx", "file"),
        ];
        for (name, kind) in cases {
            let a = bed.ask(&bed.file(name)).unwrap().attach();
            assert_eq!((a.kind.as_str(), a.name.as_str()), (kind, name));
        }
    }

    #[test]
    fn a_png_name_or_mime_cant_make_an_exe_a_picture() {
        let bed = Bed::new();
        let exe = json!({"path": bed.file("run.exe"), "name": "IMG_4250.png", "mime": "image/png"});
        assert_eq!(check(true, Actor::Person, Some(&bed.folder), &exe), Err(Refused::Kind));
        // and a picture named a pdf is still the picture its name on disk says
        let png = json!({"path": bed.file("a.png"), "name": "a.pdf", "mime": "application/pdf"});
        assert_eq!(check(true, Actor::Person, Some(&bed.folder), &png).unwrap().attach().kind, "image");
    }

    #[test]
    fn a_picture_or_pdf_still_passes_every_other_check() {
        let bed = Bed::new();
        let field = json!({"path": bed.file("a.png")});
        assert_eq!(check(false, Actor::Person, Some(&bed.folder), &field), Err(Refused::ThirdParty));
        assert_eq!(check(true, Actor::Agent, Some(&bed.folder), &field), Err(Refused::NotThePerson));
        assert_eq!(bed.ask(&bed.outside("a.pdf")), Err(Refused::Outside));
        let link = bed.folder.join("open-1f/1/link.pdf");
        symlink(bed.outside("b.pdf"), &link).unwrap();
        assert_eq!(bed.ask(&link), Err(Refused::Link));
    }

    #[test]
    fn the_fields_name_and_mime_cant_launder_a_kind_or_a_title() {
        let bed = Bed::new();
        let word = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
        let exe = json!({"path": bed.file("run.exe"), "name": "a.docx", "mime": word});
        assert_eq!(check(true, Actor::Person, Some(&bed.folder), &exe), Err(Refused::Kind));
        let doc = json!({"path": bed.file("a.docx"), "name": "../other.png", "mime": "image/png"});
        let a = check(true, Actor::Person, Some(&bed.folder), &doc).unwrap().attach();
        assert_eq!((a.kind.as_str(), a.name.as_str()), ("file", "a.docx"));
    }

    #[test]
    fn a_plugins_folder_comes_from_a_real_home_and_a_real_id() {
        let home = OsStr::new("/Users/someone");
        let want = PathBuf::from("/Users/someone/Library/Application Support/app.buwiz.fixture");
        assert_eq!(folder_of(Some(home), "app.buwiz.fixture"), Some(want));
        assert_eq!(folder_of(None, "app.buwiz.fixture"), None);
        assert_eq!(folder_of(Some(OsStr::new("")), "app.buwiz.fixture"), None);
        assert_eq!(folder_of(Some(OsStr::new("relative")), "app.buwiz.fixture"), None);
        assert_eq!(folder_of(Some(home), "../app.buwiz.fixture"), None);
        assert_eq!(folder_of(Some(home), "app/buwiz"), None);
    }
}
