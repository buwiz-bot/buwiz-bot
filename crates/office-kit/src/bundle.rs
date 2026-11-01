//! Where the app bundle's `Contents/Resources` are: beside the running
//! executable, else in the dev bundle in `~/Applications` (for a bare
//! `target/debug/buwiz-bot`), Buwiz Bot.app's and then Rox.app's, the bundle
//! before the rename.

use std::path::{Path, PathBuf};

/// The dev bundle in `~/Applications` (`app/bundle-dev.sh`).
pub const APP_BUNDLE: &str = "Buwiz Bot.app";
pub const OLD_APP_BUNDLE: &str = "Rox.app";

/// The bundle's `Contents/Resources/<sub>` beside the executable `exe`
/// (`<bundle>/Contents/MacOS/<exe>`).
pub fn exe_resources(exe: &Path, sub: &str) -> Option<PathBuf> {
    exe.parent().and_then(Path::parent).map(|contents| contents.join("Resources").join(sub))
}

/// The dev bundles' `Contents/Resources/<sub>` under `home`: Buwiz Bot.app's,
/// then Rox.app's.
pub fn installed_resources(home: &Path, sub: &str) -> Vec<PathBuf> {
    [APP_BUNDLE, OLD_APP_BUNDLE].iter().map(|b| home.join("Applications").join(b).join("Contents/Resources").join(sub)).collect()
}

/// [`exe_resources`] of the running executable, then
/// [`installed_resources`]: where to look for a bundled `<sub>`.
pub fn resources(sub: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::current_exe().ok().and_then(|exe| exe_resources(&exe, sub)).into_iter().collect();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.extend(installed_resources(Path::new(&home), sub));
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_files_are_beside_the_executable_then_in_either_dev_bundle() {
        let exe = Path::new("/X/Buwiz Bot.app/Contents/MacOS/buwiz-bot-bin");
        assert_eq!(exe_resources(exe, "fonts"), Some(PathBuf::from("/X/Buwiz Bot.app/Contents/Resources/fonts")));
        assert_eq!(installed_resources(Path::new("/Users/u"), "fonts/office"), [
            PathBuf::from("/Users/u/Applications/Buwiz Bot.app/Contents/Resources/fonts/office"),
            PathBuf::from("/Users/u/Applications/Rox.app/Contents/Resources/fonts/office"),
        ]);
        let dirs = resources("fonts");
        let exe = std::env::current_exe().unwrap();
        assert_eq!(dirs[0], exe_resources(&exe, "fonts").unwrap());
        assert_eq!(dirs.len(), 3);
        assert!(dirs[1].ends_with("Applications/Buwiz Bot.app/Contents/Resources/fonts"));
        assert!(dirs[2].ends_with("Applications/Rox.app/Contents/Resources/fonts"));
    }
}
