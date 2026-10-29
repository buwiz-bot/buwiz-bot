//! A running v1 open's life on the screen (spec 6.7, 12.2, 13.7): the
//! message an app's error shows (with Reload and Close), the opens a new
//! version leaves on old source (a Reload banner, LCY-04), and when an
//! open that lost its plugin to a restart may reopen ("Reconnecting").
//!
//! The decisions are pure; `mini.rs` and the store act on them.

use serde_json::Value;

use super::{V1Key, load::PluginEntry};

/// What a running app's error shows (6.7), from the engine's report text
/// (`AppView::set_error_handler` gives the text only): a budget trip or
/// running out of memory, else an uncaught error's first line. `None`
/// for an asset (a font) that failed: the app keeps running then.
pub fn error_message(title: &str, text: &str) -> Option<String> {
    let first = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    if first.starts_with("font failed") {
        return None;
    }
    super::load::runtime_message(title, nsh_engine::AppErrorKind::Uncaught, text)
}

/// The opens (module keys, with the fingerprint each runs) whose app now
/// has another fingerprint in `plugins`' manifests: a full-screen one keeps
/// its source and shows a Reload banner (12.2), an inline card reloads.
pub fn stale(running: &[(String, String)], plugins: &[PluginEntry]) -> Vec<String> {
    running
        .iter()
        .filter(|(key, fp)| {
            let Ok(k) = V1Key::parse(key) else { return false };
            let now = plugins
                .iter()
                .find(|p| p.id == k.plugin)
                .and_then(|p| p.app(&k.app))
                .and_then(|a| a.get("fingerprint"))
                .and_then(Value::as_str);
            now.is_some_and(|now| now != fp)
        })
        .map(|(key, _)| key.clone())
        .collect()
}

/// Whether a broker `plugin` event says `plugin` runs again: the opens
/// that showed "Reconnecting" reopen on the new generation (13.7).
pub fn running_again(event: &Value, plugin: &str) -> bool {
    let id = event.get("id").or_else(|| event.get("plugin")).and_then(Value::as_str);
    id == Some(plugin) && event.get("state").and_then(Value::as_str) == Some("running")
}

/// The apps of `plugin` whose source Rox holds (`loaded`) but that are not
/// running: after the plugin changed, they fetch its new code before they
/// show again. A running open keeps its own until Reload (LCY-04).
pub fn idle_sources<'a>(loaded: impl Iterator<Item = &'a String>, running: &[String], plugin: &str) -> Vec<String> {
    let prefix = format!("v1:{plugin}/");
    let mut keys: Vec<String> = loaded.filter(|k| k.starts_with(&prefix) && !running.contains(k)).cloned().collect();
    keys.sort();
    keys
}

/// What an open shows while its plugin restarts.
pub fn reconnecting(title: &str) -> String {
    format!("{title} is reconnecting…")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::mini_v1::records::Records;

    fn plugin(fp: &str) -> PluginEntry {
        PluginEntry::from_json(&json!({
            "id": "dev.goldcoders.fixture", "title": "Fixture", "version": "1.1.0",
            "apps": [{"id": "board", "title": "Board", "fingerprint": fp}, {"id": "card", "title": "Card", "fingerprint": "sha256:cc"}],
        }))
        .unwrap()
    }

    #[test]
    fn an_idle_card_of_a_changed_plugin_fetches_its_new_code() {
        let loaded = ["v1:dev.goldcoders.bir/card-quick?c=1", "v1:dev.goldcoders.bir/forms", "v1:dev.goldcoders.other/x", "R12"].map(String::from);
        let running = vec!["v1:dev.goldcoders.bir/forms".to_owned()];
        assert_eq!(idle_sources(loaded.iter(), &running, "dev.goldcoders.bir"), ["v1:dev.goldcoders.bir/card-quick?c=1"]);
        assert!(idle_sources(loaded.iter(), &running, "dev.goldcoders.none").is_empty());
    }

    #[test]
    fn lcy_04_a_new_fingerprint_refreshes_the_next_open_and_running_opens_keep_theirs_until_reload() {
        let running = vec![
            ("v1:dev.goldcoders.fixture/board".to_owned(), "sha256:aa".to_owned()),
            ("v1:dev.goldcoders.fixture/card".to_owned(), "sha256:cc".to_owned()),
        ];
        // same version: nothing is stale
        assert!(stale(&running, &[plugin("sha256:aa")]).is_empty());
        // the board changed: its running open is stale (a Reload banner),
        // the card is not; the open stays on its source
        assert_eq!(stale(&running, &[plugin("sha256:bb")]), ["v1:dev.goldcoders.fixture/board"]);
        // the next open loads the new source, and says so (`app_changed`)
        let mut r = Records::default();
        assert_eq!(r.opened("dev.goldcoders.fixture", "board", "sha256:aa"), None);
        assert_eq!(r.opened("dev.goldcoders.fixture", "board", "sha256:bb").as_deref(), Some("sha256:aa"));
        // a plugin that went away leaves the open as it is (its session ends)
        assert!(stale(&running, &[]).is_empty());
    }

    #[test]
    fn app_08_a_running_apps_error_shows_the_specs_message() {
        assert_eq!(error_message("Board", "TypeError: x is undefined\n    at f (app.js:3)").as_deref(), Some("Board stopped: TypeError: x is undefined."));
        assert_eq!(error_message("Board", "interrupted").as_deref(), Some("Board stopped: it took too long."));
        assert_eq!(error_message("Board", "the app ran out of memory").as_deref(), Some("Board stopped: it ran out of memory."));
        assert_eq!(error_message("Board", "font failed: no such file"), None);
    }

    #[test]
    fn reconnecting_opens_reopen_when_their_plugin_runs_again() {
        assert!(running_again(&json!({"id": "dev.goldcoders.fixture", "state": "running", "generation": 5}), "dev.goldcoders.fixture"));
        assert!(!running_again(&json!({"id": "dev.goldcoders.fixture", "state": "restarting"}), "dev.goldcoders.fixture"));
        assert!(!running_again(&json!({"id": "dev.goldcoders.other", "state": "running"}), "dev.goldcoders.fixture"));
        assert_eq!(reconnecting("Board"), "Board is reconnecting…");
    }
}
