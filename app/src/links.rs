//! The link router's decisions (`Store::open_link`), as pure functions:
//! which local files `rox://view` may show, and which outside URLs go to
//! the system without asking.
//!
//! Links reach the router from text Rox shows (model replies, MCP results,
//! library items, plugin pages) and from other apps and web pages (`open
//! rox://…`, `lib.rs`). Both are untrusted, so:
//!
//! - `rox://view?path=` shows only previews: a page, PDF, Markdown or
//!   image inside a folder of the account's own temporary folder
//!   ([`viewable`]), after the path is resolved (`..`, symlinks). Mini apps
//!   write their print previews there (the BIR host:
//!   `$TMPDIR/bir-agent-export-<uuid>/index.html`); Rox's own go in
//!   [`view_dir`].
//! - An outside URL opens unasked only for `http`, `https`, `mailto` and
//!   `tel`; any other scheme waits on Open in Rox's own sheet
//!   ([`LinkAsk`]), since it starts another app.
//! - A link from another app creates nothing (a chat) until the person
//!   says so in Rox.

use std::path::{Path, PathBuf};

/// The folder whose previews `rox://view` shows: the account's own
/// temporary folder (`$TMPDIR`, private to the account on macOS).
pub(crate) fn view_root() -> PathBuf {
    std::env::temp_dir()
}

/// Rox's own preview folder, inside [`view_root`].
pub(crate) fn view_dir() -> PathBuf {
    view_root().join("rox-view")
}

/// The kinds of file `rox://view` shows: previews, not text a temporary
/// folder may hold (settings, logs, tokens).
const PREVIEWS: &[&str] = &["html", "htm", "pdf", "md", "markdown", "png", "jpg", "jpeg", "gif", "webp", "svg"];

/// `path` resolved when it is a preview `rox://view` may show: a file of a
/// [`PREVIEWS`] kind inside a folder of `root` (files loose in `root` itself
/// are other programs' state, like the gpui-agent token).
pub(crate) fn viewable(path: &str, root: &Path) -> Option<PathBuf> {
    let real = contained(path, root)?;
    let depth = real.strip_prefix(root.canonicalize().ok()?).ok()?.components().count();
    let ext = real.extension()?.to_str()?.to_ascii_lowercase();
    (depth >= 2 && PREVIEWS.contains(&ext.as_str())).then_some(real)
}

/// Makes [`view_dir`], readable by this account only.
pub(crate) fn make_view_dir() {
    let dir = view_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return log::warn!("rox: can't make {}: {e}", dir.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
}

/// `path` resolved (`..` and symlinks gone) when it names a file inside
/// `root`; `None` for a relative path, a missing file, the folder itself,
/// or anything that resolves outside it.
pub(crate) fn contained(path: &str, root: &Path) -> Option<PathBuf> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let real = p.canonicalize().ok()?;
    (real.starts_with(&root) && real != root && real.is_file()).then_some(real)
}

/// What becomes of a URL that is no route of Rox's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outside {
    /// To the system: a web page, a mail, a call.
    Open,
    /// Another app's scheme: only after Open in Rox's sheet.
    Ask,
    /// Never: nsh's `check_url` refuses it, or it is a `rox:` URL no
    /// route took (the system would hand it straight back as an outside
    /// open).
    Refuse,
}

/// The scheme decision for a link pressed in Rox.
pub(crate) fn outside(url: &str) -> Outside {
    if nsh_core::open_url::check_url(url).is_err() {
        return Outside::Refuse;
    }
    let scheme = url.split_once(':').map(|(s, _)| s.to_ascii_lowercase()).unwrap_or_default();
    match scheme.as_str() {
        "http" | "https" | "mailto" | "tel" => Outside::Open,
        "rox" => Outside::Refuse,
        _ => Outside::Ask,
    }
}

/// A link waiting on the person in Rox's sheet (`botmodal::link_ask_layer`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkAsk {
    /// An outside URL of a scheme [`outside`] asks about.
    Open(String),
    /// A new chat with this draft, asked for by another app
    /// (`rox://chat/new?draft=`).
    NewChat(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder with `view/` (the root) holding `ok.html`, and
    /// `secret.txt` beside it.
    fn tree(test: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("rox-links-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("view/sub")).expect("dirs");
        std::fs::write(base.join("view/ok.html"), "<p>ok</p>").expect("ok");
        std::fs::write(base.join("view/sub/deep.pdf"), "%PDF").expect("deep");
        std::fs::write(base.join("secret.txt"), "secret").expect("secret");
        base
    }

    #[test]
    fn files_inside_the_view_folder_are_shown() {
        let base = tree("inside");
        let root = base.join("view");
        let ok = contained(&root.join("ok.html").to_string_lossy(), &root).expect("inside");
        assert_eq!(ok, root.join("ok.html").canonicalize().expect("real"));
        assert!(contained(&root.join("sub/deep.pdf").to_string_lossy(), &root).is_some());
        // `..` that comes back inside is fine: what counts is where it lands
        assert!(contained(&root.join("sub/../ok.html").to_string_lossy(), &root).is_some());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn dot_dot_symlink_and_prefix_escapes_are_refused() {
        let base = tree("escape");
        let root = base.join("view");
        let s = |p: PathBuf| p.to_string_lossy().into_owned();
        assert_eq!(contained(&s(root.join("../secret.txt")), &root), None);
        assert_eq!(contained(&s(root.join("sub/../../secret.txt")), &root), None);
        assert_eq!(contained("/etc/passwd", &root), None);
        assert_eq!(contained("ok.html", &root), None, "relative");
        assert_eq!(contained(&s(root.join("missing.html")), &root), None);
        assert_eq!(contained(&s(root.clone()), &root), None, "the folder itself");
        // a sibling whose name starts like the root's
        std::fs::create_dir_all(base.join("view2")).expect("view2");
        std::fs::write(base.join("view2/x.html"), "x").expect("x");
        assert_eq!(contained(&s(base.join("view2/x.html")), &root), None);
        #[cfg(unix)]
        {
            // a link inside to a file outside, and a linked folder
            std::os::unix::fs::symlink(base.join("secret.txt"), root.join("link.html")).expect("link");
            std::os::unix::fs::symlink(&base, root.join("up")).expect("dir link");
            assert_eq!(contained(&s(root.join("link.html")), &root), None);
            assert_eq!(contained(&s(root.join("up/secret.txt")), &root), None);
            // a link inside to a file inside is that file
            std::os::unix::fs::symlink(root.join("ok.html"), root.join("alias.html")).expect("alias");
            assert!(contained(&s(root.join("alias.html")), &root).is_some());
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_root_reached_through_a_symlink_still_contains_its_files() {
        // macOS's /var is /private/var: the root and the path resolve alike
        let base = tree("rootlink");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(base.join("view"), base.join("via")).expect("root link");
            let root = base.join("via");
            assert!(contained(&base.join("view/ok.html").to_string_lossy(), &root).is_some());
            assert!(contained(&root.join("ok.html").to_string_lossy(), &root).is_some());
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn only_previews_in_a_folder_of_the_root_are_viewable() {
        let base = tree("viewable");
        let root = base.join("view");
        let s = |p: PathBuf| p.to_string_lossy().into_owned();
        std::fs::create_dir_all(root.join("bir-agent-export-1")).expect("export");
        std::fs::write(root.join("bir-agent-export-1/index.html"), "<p>sheet</p>").expect("sheet");
        std::fs::write(root.join("sub/notes.txt"), "text").expect("txt");
        std::fs::write(root.join("sub/conf.json"), "{}").expect("json");
        std::fs::write(root.join("token"), "secret").expect("token");
        assert!(viewable(&s(root.join("bir-agent-export-1/index.html")), &root).is_some());
        assert!(viewable(&s(root.join("sub/deep.pdf")), &root).is_some());
        // loose in the root (another program's state), or not a preview
        assert_eq!(viewable(&s(root.join("ok.html")), &root), None);
        assert_eq!(viewable(&s(root.join("token")), &root), None);
        assert_eq!(viewable(&s(root.join("sub/notes.txt")), &root), None);
        assert_eq!(viewable(&s(root.join("sub/conf.json")), &root), None);
        // and everything `contained` refuses
        assert_eq!(viewable(&s(root.join("../secret.txt")), &root), None);
        assert_eq!(viewable("/etc/hosts", &root), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn web_mail_and_phone_links_open_and_other_apps_ask() {
        for url in [
            "https://example.com/x",
            "http://example.com",
            "HTTPS://Example.com",
            "mailto:ada@example.com",
            "tel:+15551234",
        ] {
            assert_eq!(outside(url), Outside::Open, "{url}");
        }
        for url in [
            "ssh://host",
            "x-apple.systempreferences:com.apple.preference",
            "vscode://file/etc/passwd",
            "smb://server/share",
            "ftp://example.com",
            "facetime:ada@example.com",
        ] {
            assert_eq!(outside(url), Outside::Ask, "{url}");
        }
    }

    #[test]
    fn refused_schemes_rox_leftovers_and_malformed_urls_never_open() {
        for url in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,x",
            "rox://chat/",
            "rox://unknown",
            "https://",
            "https://example.com/a b",
            "not a url",
            "",
        ] {
            assert_eq!(outside(url), Outside::Refuse, "{url:?}");
        }
    }
}
