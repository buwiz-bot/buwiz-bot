//! `_meta["buwiz/document"] = {path, name, mime}`: a file a first-party
//! plugin hands to the Bot's own Word or Spreadsheet viewer. No permission
//! key grants this and the plugin's word proves nothing, so [`check`] decides
//! from what the Bot knows and the disk says. Its checks go in the contract's
//! order: first-party, the person's own press, a real file in that plugin's
//! own folder, a kind the viewer takes. What passes opens by the path found
//! on disk, under the name found there: the field's `name` and `mime` are
//! never read, so they can't launder a kind.
//!
//! A refusal opens nothing and carries no part of the path: [`Refused`] has
//! no data, so a log line can't hold one.
//!
//! **What this can't do.** The viewer opens by path, so a plugin that swaps a
//! checked file before the viewer reads it wins that race. It would have to
//! be first-party and racing a file in its own folder.

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
    /// Nothing there.
    Missing,
    /// There, but no regular file.
    NotAFile,
    /// A kind the viewer doesn't take.
    Kind,
}

/// A file that passed [`check`]: its real path, and the name it has there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    path: String,
    name: String,
}

impl Document {
    /// What `store.open_file` takes: `file` lets the viewer route by the
    /// extension, and no plugin's word can make it a lightbox's image.
    pub fn attach(&self) -> Attach {
        Attach {
            kind: "file".to_owned(),
            path: self.path.clone(),
            name: self.name.clone(),
            ext: crate::files::ext_of(&self.name, ""),
            ..Default::default()
        }
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
    if consent::ask(consent::inline_card(first_party)) != Ask::Nothing {
        return Err(Refused::ThirdParty);
    }
    if actor != Actor::Person {
        return Err(Refused::NotThePerson);
    }
    let claimed = field.get("path").and_then(Value::as_str).ok_or(Refused::BadField)?;
    let real = file_in(folder.ok_or(Refused::NoFolder)?, Path::new(claimed))?;
    let path = real.to_str().ok_or(Refused::Kind)?.to_owned();
    if !crate::office::native_ext(&path) {
        return Err(Refused::Kind);
    }
    let name = real.file_name().and_then(OsStr::to_str).ok_or(Refused::Kind)?.to_owned();
    Ok(Document { path, name })
}

/// `claimed` as the real path of a regular file inside `folder`, with no
/// symlink from the folder down. Canonicalizing alone would follow a link
/// that stays inside, so each step is looked at unfollowed. What is above
/// the folder is the account's own: only that part may be a link (`/var`
/// is one), and `claimed` may spell it either way.
fn file_in(folder: &Path, claimed: &Path) -> Result<PathBuf, Refused> {
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
    // a second name for a file elsewhere: ⌘S would write through it
    if std::os::unix::fs::MetadataExt::nlink(&meta) != 1 {
        return Err(Refused::Link);
    }
    let real = fs::canonicalize(&at).map_err(|_| Refused::Missing)?;
    if real != at || !real.starts_with(&root) {
        return Err(Refused::Outside);
    }
    Ok(real)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use serde_json::json;

    use super::*;

    /// A plugin's folder with `open/1/` in it, and a folder beside it.
    struct Bed {
        base: PathBuf,
        folder: PathBuf,
    }

    impl Bed {
        fn new() -> Bed {
            let base = crate::mini_v1::storage::scratch_root("document");
            let folder = base.join("app.buwiz.fixture");
            fs::create_dir_all(folder.join("open/1")).unwrap();
            fs::create_dir_all(base.join("elsewhere")).unwrap();
            Bed { base, folder }
        }

        /// Writes `open/1/<name>`: its path as the plugin would send it.
        fn file(&self, name: &str) -> PathBuf {
            let path = self.folder.join("open/1").join(name);
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
            let real = fs::canonicalize(bed.folder.join("open/1").join(name)).unwrap();
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
        let literal = folder.join("open/1/a.docx");
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
        assert_eq!(bed.ask(Path::new("open/1/a.docx")), Err(Refused::Outside), "relative");
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
        assert_eq!(bed.ask(&bed.folder.join("open/../../elsewhere/a.docx")), Err(Refused::Outside));
        assert_eq!(bed.ask(&bed.folder.join("../elsewhere/a.docx")), Err(Refused::Outside));
        // it stays inside, and still isn't what the plugin was told to send
        assert_eq!(bed.ask(&bed.folder.join("open/1/../1/b.docx")), Err(Refused::Outside));
    }

    #[test]
    fn a_symlinked_file_opens_nothing_wherever_it_points() {
        let bed = Bed::new();
        let link = bed.folder.join("open/1/out.docx");
        symlink(bed.outside("a.docx"), &link).unwrap();
        assert_eq!(bed.ask(&link), Err(Refused::Link));
        let inner = bed.folder.join("open/1/in.docx");
        symlink(bed.file("b.docx"), &inner).unwrap();
        assert_eq!(bed.ask(&inner), Err(Refused::Link), "a link that stays inside is still a link");
    }

    #[test]
    fn a_symlinked_folder_on_the_way_opens_nothing_wherever_it_points() {
        let bed = Bed::new();
        bed.outside("a.docx");
        symlink(bed.base.join("elsewhere"), bed.folder.join("open/2")).unwrap();
        assert_eq!(bed.ask(&bed.folder.join("open/2/a.docx")), Err(Refused::Link));
        bed.file("b.docx");
        symlink(bed.folder.join("open/1"), bed.folder.join("open/3")).unwrap();
        assert_eq!(bed.ask(&bed.folder.join("open/3/b.docx")), Err(Refused::Link));
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
        let link = bed.folder.join("open/1/a.docx");
        fs::hard_link(bed.outside("a.docx"), &link).unwrap();
        assert_eq!(bed.ask(&link), Err(Refused::Link));
    }

    #[test]
    fn a_directory_or_a_missing_file_opens_nothing() {
        let bed = Bed::new();
        let dir = bed.folder.join("open/1/dir.docx");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(bed.ask(&dir), Err(Refused::NotAFile));
        assert_eq!(bed.ask(&bed.folder), Err(Refused::NotAFile), "the folder itself");
        assert_eq!(bed.ask(&bed.folder.join("open/1/gone.docx")), Err(Refused::Missing));
        // a file where a folder should be
        let file = bed.file("a.docx");
        assert_eq!(bed.ask(&file.join("b.docx")), Err(Refused::Missing));
    }

    #[test]
    fn a_kind_the_viewer_does_not_take_opens_nothing() {
        let bed = Bed::new();
        for name in ["run.exe", "a.docx.exe", "a.pptx", "a.pdf", "a.unknown", "noextension", ".docx"] {
            assert_eq!(bed.ask(&bed.file(name)), Err(Refused::Kind), "{name}");
        }
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
