//! A running v1 open's life on the screen (spec 6.7, 12.2, 13.7): the
//! message an app's error shows (with Reload and Close), the opens a new
//! version leaves on old source (a Reload banner, LCY-04; an inline card
//! keeps what it shows and says "Made with 1.4.0 · Refresh"), and when an
//! open that lost its plugin to a restart may reopen ("Reconnecting").
//!
//! The decisions are pure; `mini.rs` and the store act on them.

use std::collections::HashSet;

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
/// its source and shows a Reload banner (12.2). An inline card keeps what
/// it shows too: nothing reloads it but its own Refresh ([`made_with`]).
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

/// What an inline card's footer says once its plugin's version is no
/// longer the one it was `made` with ("Made with 1.4.0", beside Refresh):
/// the card keeps its content until the person refreshes it. `None` while
/// the version is the same, or either is unknown.
pub fn made_with(made: &str, now: &str) -> Option<String> {
    let (made, now) = (super::connect::bare_version(made), super::connect::bare_version(now));
    (!made.is_empty() && !now.is_empty() && made != now).then(|| format!("Made with {made}"))
}

/// After `plugin` restarted: of the opens waiting on it (`waiting`, each
/// module key with whether it is an inline card and the version it was
/// made with), the ones that reopen on the new process. A full-screen
/// open always does; an inline card does when the plugin is the version
/// it was made with (a crash, a relaunch: it comes back as if nothing
/// happened), and stays as it is after an update, until its Refresh.
pub fn reopen_after_restart(waiting: &[(String, bool, String)], now: &str) -> (Vec<String>, Vec<String>) {
    let (keep, reopen): (Vec<_>, Vec<_>) = waiting.iter().partition(|(_, inline, made)| *inline && made_with(made, now).is_some());
    (reopen.into_iter().map(|(k, _, _)| k.clone()).collect(), keep.into_iter().map(|(k, _, _)| k.clone()).collect())
}

/// The opens waiting on a restart when the plugin list could not be read
/// (`keys`), split into the ones that wait for the next list and the ones
/// that reopen now: an inline card made with a known version (`made`)
/// waits, since only the list can say whether an update must leave it as
/// it is; a full-screen open (`full`) or a card of unknown version reopens.
pub fn undecided_restarts(keys: Vec<String>, full: &HashSet<String>, made: &HashSet<String>) -> (Vec<String>, Vec<String>) {
    keys.into_iter().partition(|k| !full.contains(k) && made.contains(k))
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

/// Whether a broker `plugin` event repeats the last one Rox heard for its
/// plugin (the same state, code, generation, enabled and waiting
/// update): no news, so no list and no reload. `last` keeps what each plugin last said.
pub fn repeated(last: &mut std::collections::HashMap<String, String>, event: &Value) -> bool {
    let id = event.get("id").or_else(|| event.get("plugin")).and_then(Value::as_str).unwrap_or_default();
    let said = [&event["state"], &event["code"], &event["generation"], &event["enabled"], &event["update"]].map(|v| v.to_string()).join(" ");
    last.insert(id.to_owned(), said.clone()).is_some_and(|before| before == said)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::mini_v1::records::Records;

    fn plugin(fp: &str) -> PluginEntry {
        PluginEntry::from_json(&json!({
            "id": "app.buwiz.fixture", "title": "Fixture", "version": "1.1.0",
            "apps": [{"id": "board", "title": "Board", "fingerprint": fp}, {"id": "card", "title": "Card", "fingerprint": "sha256:cc"}],
        }))
        .unwrap()
    }

    #[test]
    fn an_idle_card_of_a_changed_plugin_fetches_its_new_code() {
        let loaded = ["v1:app.buwiz.ebirforms/card-quick?c=1", "v1:app.buwiz.ebirforms/forms", "v1:app.buwiz.other/x", "R12"].map(String::from);
        let running = vec!["v1:app.buwiz.ebirforms/forms".to_owned()];
        assert_eq!(idle_sources(loaded.iter(), &running, "app.buwiz.ebirforms"), ["v1:app.buwiz.ebirforms/card-quick?c=1"]);
        assert!(idle_sources(loaded.iter(), &running, "app.buwiz.none").is_empty());
    }

    #[test]
    fn lcy_04_a_new_fingerprint_refreshes_the_next_open_and_running_opens_keep_theirs_until_reload() {
        let running = vec![
            ("v1:app.buwiz.fixture/board".to_owned(), "sha256:aa".to_owned()),
            ("v1:app.buwiz.fixture/card".to_owned(), "sha256:cc".to_owned()),
        ];
        // same version: nothing is stale
        assert!(stale(&running, &[plugin("sha256:aa")]).is_empty());
        // the board changed: its running open is stale (a Reload banner),
        // the card is not; the open stays on its source
        assert_eq!(stale(&running, &[plugin("sha256:bb")]), ["v1:app.buwiz.fixture/board"]);
        // the next open loads the new source, and says so (`app_changed`)
        let mut r = Records::default();
        assert_eq!(r.opened("app.buwiz.fixture", "board", "sha256:aa"), None);
        assert_eq!(r.opened("app.buwiz.fixture", "board", "sha256:bb").as_deref(), Some("sha256:aa"));
        // a plugin that went away leaves the open as it is (its session ends)
        assert!(stale(&running, &[]).is_empty());
    }

    #[test]
    fn an_inline_card_keeps_its_content_across_an_update_and_reopens_after_a_plain_restart() {
        assert_eq!(made_with("1.4.0", "1.5.0").as_deref(), Some("Made with 1.4.0"));
        assert_eq!(made_with("v1.4.0", "1.4.0"), None, "the same version, however it is written");
        assert_eq!(made_with("", "1.5.0"), None);
        assert_eq!(made_with("1.4.0", ""), None, "a plugin that is not listed says nothing");
        let waiting = vec![
            ("v1:p/card".to_owned(), true, "1.4.0".to_owned()),
            ("v1:p/board".to_owned(), false, "1.4.0".to_owned()),
            ("v1:p/other".to_owned(), true, String::new()),
        ];
        // a crash or a relaunch: every open comes back on the new process
        let (reopen, keep) = reopen_after_restart(&waiting, "1.4.0");
        assert_eq!(reopen, ["v1:p/card", "v1:p/board", "v1:p/other"]);
        assert!(keep.is_empty());
        // an update: the inline card stays as it is until its Refresh; the
        // full screen reconnects as it did, and a card of unknown version
        // is not left dead
        let (reopen, keep) = reopen_after_restart(&waiting, "1.5.0");
        assert_eq!(reopen, ["v1:p/board", "v1:p/other"]);
        assert_eq!(keep, ["v1:p/card"]);
        // the list can't be read: only the inline card of a known version
        // waits for the next one, since only it can say there was an update
        let keys = waiting.iter().map(|(k, _, _)| k.clone()).collect();
        let full = HashSet::from(["v1:p/board".to_owned()]);
        let made = HashSet::from(["v1:p/card".to_owned(), "v1:p/board".to_owned()]);
        let (wait, reopen) = undecided_restarts(keys, &full, &made);
        assert_eq!(wait, ["v1:p/card"]);
        assert_eq!(reopen, ["v1:p/board", "v1:p/other"]);
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
        assert!(running_again(&json!({"id": "app.buwiz.fixture", "state": "running", "generation": 5}), "app.buwiz.fixture"));
        assert!(!running_again(&json!({"id": "app.buwiz.fixture", "state": "restarting"}), "app.buwiz.fixture"));
        assert!(!running_again(&json!({"id": "app.buwiz.other", "state": "running"}), "app.buwiz.fixture"));
        assert_eq!(reconnecting("Board"), "Board is reconnecting…");
    }

    #[test]
    fn a_plugin_event_that_says_nothing_new_is_a_repeat() {
        use serde_json::json;
        let mut last = std::collections::HashMap::new();
        let running = json!({"id": "p", "state": "running", "code": null, "generation": 1, "enabled": true});
        assert!(!super::repeated(&mut last, &running), "the first is news");
        assert!(super::repeated(&mut last, &running), "the same again is not");
        assert!(super::repeated(&mut last, &json!({"id": "p", "state": "running", "code": null, "generation": 1, "enabled": true, "seq": 9})), "a new seq alone is not");
        assert!(!super::repeated(&mut last, &json!({"id": "p", "state": "running", "code": null, "generation": 2, "enabled": true})), "a new generation is");
        assert!(!super::repeated(&mut last, &json!({"id": "q", "state": "running", "code": null, "generation": 2, "enabled": true})), "another plugin's is its own");
        assert!(!super::repeated(&mut last, &json!({"id": "p", "state": "stopped", "code": null, "generation": 2, "enabled": true})), "a new state is");
        assert!(!super::repeated(&mut last, &json!({"id": "p", "state": "stopped", "code": null, "generation": 2, "enabled": true, "update": "1.5.0"})), "an update found is");
        assert!(super::repeated(&mut last, &json!({"id": "p", "state": "stopped", "code": null, "generation": 2, "enabled": true, "update": "1.5.0"})));
        assert!(!super::repeated(&mut last, &json!({"id": "p", "state": "stopped", "code": null, "generation": 2, "enabled": true, "update": null})), "and one installed is");
    }
}
