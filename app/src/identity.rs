//! The app's identity on this Mac: its bundle id, its bundle, and the
//! folders it keeps data, logs and caches in. Rox became Buwiz Bot
//! (`com.buwiz.bot`); what a Rox install kept is moved over once
//! ([`migrate_data`]), so nothing is lost. Rox's releases were withdrawn:
//! there is no going back to them, and no compatibility path stays.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The bundle id, and the nsh app id (its storage folder is named after it).
pub const APP_ID: &str = "com.buwiz.bot";
/// The mini apps' views (`mini.rs`).
pub const MINI_APP_ID: &str = "com.buwiz.bot.mini";
pub const MINI_V1_APP_ID: &str = "com.buwiz.bot.mini.v1";
/// The bundle id before the rename.
pub const OLD_APP_ID: &str = "dev.goldcoders.rox";

/// The folder name in Application Support, Logs and Caches.
pub const DIR_NAME: &str = "Buwiz Bot";
pub const OLD_DIR_NAME: &str = "Rox";
/// The dev bundle in `~/Applications` (`app/bundle-dev.sh`); the office
/// views find its `Contents/Resources` (`buwiz_office_kit::bundle`).
pub use buwiz_office_kit::bundle::{APP_BUNDLE, OLD_APP_BUNDLE};
/// Written in the data dir once Rox's was moved or copied into it.
pub const MIGRATED_MARKER: &str = ".migrated-from-rox";

const SUPPORT: &str = "Library/Application Support";

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Where the app keeps its data: `ROX_DATA_DIR`, else
/// `~/Library/Application Support/Buwiz Bot`, or Rox's folder while a
/// move that could not run left the data there (it is tried again at the
/// next launch).
pub fn data_dir() -> PathBuf {
    data_dir_in(std::env::var_os("ROX_DATA_DIR").as_deref(), &home())
}

/// [`data_dir`] with the override `over` and the home `home`.
pub fn data_dir_in(over: Option<&OsStr>, home: &Path) -> PathBuf {
    if let Some(d) = over.filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let new = home.join(SUPPORT).join(DIR_NAME);
    let old = home.join(SUPPORT).join(OLD_DIR_NAME);
    if !new.exists() && old.is_dir() { old } else { new }
}

/// Where nsh keeps the app's own data (`storage.json`, under `<dir>/<app
/// id>`) for an instance with its own data dir (`ROX_DATA_DIR`, which
/// scripts/test-rox sets): `<ROX_DATA_DIR>/nsh`, so a test instance never
/// shares the owner's `~/Library/Application Support/com.buwiz.bot`.
/// `None` (the owner's app) leaves nsh's default.
pub fn nsh_data_dir_in(over: Option<&OsStr>) -> Option<PathBuf> {
    over.filter(|d| !d.is_empty()).map(|d| PathBuf::from(d).join("nsh"))
}

/// `~/Library/Logs/Buwiz Bot` (the broker's and the jank log).
pub fn logs_dir(home: &Path) -> PathBuf {
    home.join("Library/Logs").join(DIR_NAME)
}

/// What [`migrate_data`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Migration {
    /// `ROX_DATA_DIR` is set: nothing is looked at.
    Overridden,
    /// No Rox data dir.
    Nothing,
    /// The new dir is there already (migrated before, or both present):
    /// neither is touched.
    AlreadyThere,
    /// Renamed.
    Moved,
    /// Copied, then the old dir removed (the rename failed).
    Copied,
}

/// Moves Rox's data dir to Buwiz Bot's once, on first launch: when the
/// new one is missing and the old one is a folder. A rename (same volume)
/// keeps every file as it was, `mini-v1.json` (attachments, pins,
/// `migrated_at`) with it. Should the rename fail it
/// copies instead, into a side folder renamed into place when complete (a
/// half copy is never taken for the data), and only then removes the old
/// one. Then it writes [`MIGRATED_MARKER`]. Once the new dir exists
/// nothing is looked at again. nsh's storage of the app id
/// (`Application Support/<id>`) moves the same way.
pub fn migrate_data(over: Option<&OsStr>, home: &Path) -> Result<Migration, String> {
    migrate_data_with(over, home, &|from, to| std::fs::rename(from, to))
}

type Rename = dyn Fn(&Path, &Path) -> std::io::Result<()>;

fn migrate_data_with(over: Option<&OsStr>, home: &Path, rename: &Rename) -> Result<Migration, String> {
    if over.is_some_and(|d| !d.is_empty()) {
        return Ok(Migration::Overridden);
    }
    let support = home.join(SUPPORT);
    let done = migrate_dir(&support.join(OLD_DIR_NAME), &support.join(DIR_NAME), rename)?;
    // nsh's storage of the root's app id; none is kept today, so a failure
    // here is no reason to stop
    if let Err(e) = migrate_dir(&support.join(OLD_APP_ID), &support.join(APP_ID), rename) {
        log::warn!("rox: {OLD_APP_ID}'s storage was not moved: {e}");
    }
    Ok(done)
}

/// [`migrate_data`] for one folder.
fn migrate_dir(old: &Path, new: &Path, rename: &Rename) -> Result<Migration, String> {
    if new.symlink_metadata().is_ok() {
        return Ok(Migration::AlreadyThere);
    }
    if !old.symlink_metadata().is_ok_and(|m| m.is_dir()) {
        return Ok(Migration::Nothing);
    }
    let how = match rename(old, new) {
        Ok(()) => Migration::Moved,
        Err(_) => {
            let name = new.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let partial = new.with_file_name(format!("{name}.partial"));
            let _ = std::fs::remove_dir_all(&partial);
            copy_tree(old, &partial).map_err(|e| format!("copying {} to {}: {e}", old.display(), partial.display()))?;
            std::fs::rename(&partial, new).map_err(|e| format!("{}: {e}", new.display()))?;
            // the copy is complete and in place: the old dir has nothing left to give
            if let Err(e) = std::fs::remove_dir_all(old) {
                log::warn!("rox: {} was copied but not removed: {e}", old.display());
            }
            Migration::Copied
        }
    };
    let marker = serde_json::json!({
        "from": old.to_string_lossy(),
        "how": if how == Migration::Moved { "moved" } else { "copied" },
        "at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
    });
    std::fs::write(new.join(MIGRATED_MARKER), format!("{marker}\n")).map_err(|e| format!("{}: {e}", new.join(MIGRATED_MARKER).display()))?;
    Ok(how)
}

/// `from` copied to `to`: folders, files (with their permissions) and
/// symlinks as links.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir(to)?;
    std::fs::set_permissions(to, std::fs::metadata(from)?.permissions())?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&src)?, &dst)?;
        } else if kind.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn a_test_instance_keeps_nsh_storage_in_its_own_data_dir() {
        assert_eq!(nsh_data_dir_in(Some(OsStr::new("/tmp/bench/data"))), Some(PathBuf::from("/tmp/bench/data/nsh")));
        assert_eq!(nsh_data_dir_in(Some(OsStr::new(""))), None, "an empty override is none");
        assert_eq!(nsh_data_dir_in(None), None, "the owner's app keeps nsh's default");
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rox-identity-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(SUPPORT)).unwrap();
        d
    }

    /// A Rox data dir as a migrated install has it: the v0 migration done
    /// at `migrated_at` 1234, a logins table, a plugin's files.
    fn rox_dir(home: &Path) -> PathBuf {
        let old = home.join(SUPPORT).join(OLD_DIR_NAME);
        std::fs::create_dir_all(old.join("plugins/dev.example.p")).unwrap();
        std::fs::write(old.join("mini-v1.json"), r#"{"attachments":[],"pins":[],"opened":[],"migrated_at":1234,"sent":[],"versions":[]}"#).unwrap();
        std::fs::write(old.join("logins.db"), b"sqlite").unwrap();
        std::fs::write(old.join("plugins/dev.example.p/manifest.json"), b"{}").unwrap();
        old
    }

    fn new_dir(home: &Path) -> PathBuf {
        home.join(SUPPORT).join(DIR_NAME)
    }

    /// The mini-app records came over as they were, `migrated_at` with them.
    fn records_kept(dir: &Path) {
        assert_eq!(crate::mini_v1::records::Records::load(dir).migrated_at, 1234);
    }

    #[test]
    fn rox_data_moves_to_buwiz_bot_once_with_a_marker() {
        let home = scratch("move");
        let old = rox_dir(&home);
        assert_eq!(data_dir_in(None, &home), old, "before the migration the data is Rox's");
        assert_eq!(migrate_data(None, &home).unwrap(), Migration::Moved);
        let new = new_dir(&home);
        assert_eq!(data_dir_in(None, &home), new);
        assert_eq!(std::fs::read(new.join("logins.db")).unwrap(), b"sqlite");
        assert!(new.join("plugins/dev.example.p/manifest.json").is_file());
        assert!(new.join(MIGRATED_MARKER).is_file());
        assert!(old.symlink_metadata().is_err(), "no compatibility path is left");
        records_kept(&new);
        // once only
        assert_eq!(migrate_data(None, &home).unwrap(), Migration::AlreadyThere);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_failed_rename_copies_then_removes_rox_data() {
        let home = scratch("copy");
        let old = rox_dir(&home);
        let refuse = |_: &Path, _: &Path| -> std::io::Result<()> { Err(std::io::Error::other("cross-device link")) };
        assert_eq!(migrate_data_with(None, &home, &refuse).unwrap(), Migration::Copied);
        let new = new_dir(&home);
        assert!(!old.exists(), "the old dir goes once the copy is in place");
        assert_eq!(std::fs::read(new.join("logins.db")).unwrap(), b"sqlite");
        assert!(new.join("plugins/dev.example.p/manifest.json").is_file());
        assert!(new.join(MIGRATED_MARKER).is_file());
        assert!(!home.join(SUPPORT).join(format!("{DIR_NAME}.partial")).exists());
        assert_eq!(data_dir_in(None, &home), new);
        records_kept(&new);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn both_dirs_present_are_left_alone() {
        let home = scratch("both");
        let old = rox_dir(&home);
        let new = new_dir(&home);
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("logins.db"), b"newer").unwrap();
        assert_eq!(migrate_data(None, &home).unwrap(), Migration::AlreadyThere);
        assert_eq!(std::fs::read(new.join("logins.db")).unwrap(), b"newer");
        assert_eq!(std::fs::read(old.join("logins.db")).unwrap(), b"sqlite");
        assert!(!new.join(MIGRATED_MARKER).exists());
        assert_eq!(data_dir_in(None, &home), new);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn rox_data_dir_overrides_and_nothing_moves() {
        let home = scratch("override");
        let old = rox_dir(&home);
        let own = home.join("own");
        assert_eq!(migrate_data(Some(own.as_os_str()), &home).unwrap(), Migration::Overridden);
        assert_eq!(data_dir_in(Some(own.as_os_str()), &home), own);
        assert!(old.join("logins.db").is_file() && !new_dir(&home).exists());
        // an empty value is no override
        assert_eq!(data_dir_in(Some(OsStr::new("")), &home), old);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_fresh_install_has_nothing_to_move() {
        let home = scratch("fresh");
        assert_eq!(migrate_data(None, &home).unwrap(), Migration::Nothing);
        assert_eq!(data_dir_in(None, &home), new_dir(&home));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn nsh_storage_of_the_old_app_id_moves_too() {
        let home = scratch("nsh");
        let old = home.join(SUPPORT).join(OLD_APP_ID);
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("storage.json"), b"{}").unwrap();
        migrate_data(None, &home).unwrap();
        assert!(home.join(SUPPORT).join(APP_ID).join("storage.json").is_file());
        let _ = std::fs::remove_dir_all(&home);
    }

    /// `app/bundle-dev.sh` against a scratch bundle, with stand-in
    /// binaries: the bundle has the new id, the buwiz scheme alone and the new
    /// executable. Skipped (said so) without the icon font or server/'s
    /// node_modules, which the script copies.
    #[test]
    fn bundle_dev_writes_buwiz_bots_identity() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let real_home = home();
        let font = ["Buwiz Bot", "Rox"]
            .iter()
            .map(|d| real_home.join("Library/Caches").join(d).join("fonts/MaterialSymbolsOutlined-v382.ttf"))
            .find(|f| f.is_file());
        let modules = manifest.join("../server/node_modules/@betteroffice");
        let (Some(font), true) = (font, modules.join("fonts/assets/Carlito-Regular.ttf").is_file()) else {
            eprintln!("skipped: no cached Material Symbols font or no server/node_modules");
            return;
        };
        let dir = scratch("bundle");
        let fake = dir.join("fake-bin");
        std::fs::write(&fake, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let app = dir.join(APP_BUNDLE);
        let out = std::process::Command::new("bash")
            .arg(manifest.join("bundle-dev.sh"))
            .env("HOME", &dir)
            .env("ROX_APP", &app)
            .env("MATERIAL_SYMBOLS_TTF", &font)
            .env("OFFICE_FONTS", modules.join("fonts"))
            .env("OFFICE_DIST", dir.join("no-dist"))
            .env("BUWIZ_GWS_MANIFEST", dir.join("no-manifest"))
            .env("BUWIZ_BROKER_BIN", &fake)
            .env("BUWIZ_GWS_BIN", &fake)
            .env("NSH_BIN", &fake)
            .env("NSH_HOST_BIN", &fake)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let plist = std::process::Command::new("/usr/bin/plutil")
            .args(["-convert", "json", "-o", "-"])
            .arg(app.join("Contents/Info.plist"))
            .output()
            .unwrap();
        let plist: serde_json::Value = serde_json::from_slice(&plist.stdout).unwrap();
        assert_eq!(plist["CFBundleIdentifier"], APP_ID);
        assert_eq!(plist["CFBundleExecutable"], "buwiz-bot");
        assert_eq!(plist["CFBundleDisplayName"], "Buwiz Bot");
        assert_eq!(plist["CFBundleURLTypes"][0]["CFBundleURLName"], APP_ID);
        assert_eq!(plist["CFBundleURLTypes"][0]["CFBundleURLSchemes"], serde_json::json!(["buwiz"]), "rox:// is not routed");
        let launcher = app.join("Contents/MacOS/buwiz-bot");
        assert!(std::fs::metadata(&launcher).unwrap().permissions().mode() & 0o111 != 0);
        assert!(std::fs::read_to_string(&launcher).unwrap().contains("/buwiz-bot-bin\""));
        assert!(!app.join("Contents/MacOS/rox").exists());
        // a dry run leaves LaunchServices alone
        assert!(!String::from_utf8_lossy(&out.stdout).contains("unregistered"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn logs_go_to_buwiz_bots_folder() {
        assert_eq!(logs_dir(Path::new("/Users/u")), PathBuf::from("/Users/u/Library/Logs/Buwiz Bot"));
    }

    #[test]
    fn the_app_id_is_buwiz_bots_everywhere_it_is_written() {
        assert_eq!(APP_ID, "com.buwiz.bot");
        assert_eq!((MINI_APP_ID, MINI_V1_APP_ID), ("com.buwiz.bot.mini", "com.buwiz.bot.mini.v1"));
        let nsh: serde_json::Value = serde_json::from_str(include_str!("../nsh.json")).unwrap();
        assert_eq!(nsh["id"], APP_ID);
        assert!(include_str!("../ios.sh").contains(&format!("\nID={APP_ID}\n")));
        assert!(include_str!("main.rs").contains(".app_id(rox::identity::APP_ID)"));
        assert!(include_str!("main.rs").contains("app_id: rox::identity::APP_ID.to_owned()"));
    }
}
