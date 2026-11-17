//! Buwiz Bot's environment variables by their Buwiz names, the old Rox and
//! Grok names still read. [`NAMES`] is the whole table, (new, old); a test
//! here pins it in tests/pins/env_names.json.
//!
//! A read takes the new name when it has a value, else the old one when it
//! is set (an empty new name gives way to a non-empty old one), and then
//! says once on stderr `<old> is an old name; use <new>`. Whoever
//! sets a variable for a child that may be an older build sets both names
//! ([`with_old`]).

use std::collections::HashSet;
use std::ffi::OsString;
use std::sync::Mutex;

/// Every renamed variable: (new name, old name).
pub const NAMES: &[(&str, &str)] = &[
    // the app
    ("BUWIZ_BACKEND", "ROX_BACKEND"),
    ("BUWIZ_BACKGROUND", "ROX_BACKGROUND"),
    ("BUWIZ_DATA_DIR", "ROX_DATA_DIR"),
    ("BUWIZ_HOST_TOKEN_FILE", "ROX_HOST_TOKEN_FILE"),
    ("BUWIZ_JANK_LOG", "ROX_JANK_LOG"),
    ("BUWIZ_FPS", "ROX_FPS"),
    ("BUWIZ_MACHINE_ID", "ROX_MACHINE_ID"),
    ("BUWIZ_MINI_CACHE", "ROX_MINI_CACHE"),
    ("BUWIZ_OFFICE_FONTS", "ROX_OFFICE_FONTS"),
    // the app's debug knobs (app/src/hot.rs)
    ("BUWIZ_STATUS", "ROX_STATUS"),
    ("BUWIZ_PANE", "ROX_PANE"),
    ("BUWIZ_PLUGINS", "ROX_PLUGINS"),
    ("BUWIZ_MODELS", "ROX_MODELS"),
    ("BUWIZ_SIDE", "ROX_SIDE"),
    ("BUWIZ_ACCOUNT", "ROX_ACCOUNT"),
    ("BUWIZ_TAB", "ROX_TAB"),
    ("BUWIZ_AVATAR", "ROX_AVATAR"),
    ("BUWIZ_OPEN", "ROX_OPEN"),
    ("BUWIZ_SCROLL", "ROX_SCROLL"),
    ("BUWIZ_SEND", "ROX_SEND"),
    ("BUWIZ_PALETTE", "ROX_PALETTE"),
    ("BUWIZ_SLOTS", "ROX_SLOTS"),
    ("BUWIZ_MINI", "ROX_MINI"),
    ("BUWIZ_CARD", "ROX_CARD"),
    ("BUWIZ_GAZE", "ROX_GAZE"),
    ("BUWIZ_PILL", "ROX_PILL"),
    ("BUWIZ_OVERLAY", "ROX_OVERLAY"),
    // the broker
    ("BUWIZ_BROKER_DATA_DIR", "GROK_DATA_DIR"),
    ("BUWIZ_ALLOWED_ORIGINS", "GROK_ALLOWED_ORIGINS"),
    ("BUWIZ_BUNDLE_ID", "GROK_BUNDLE_ID"),
    ("BUWIZ_PUBLIC_PORT", "ROX_PUBLIC_PORT"),
    ("BUWIZ_PLUGIN_CATALOG", "ROX_PLUGIN_CATALOG"),
    ("BUWIZ_V1_CALL_TIMEOUT_MS", "ROX_V1_CALL_TIMEOUT_MS"),
    // read by bridge/rox-bridge.mjs, which keeps its own copy of the read rule
    ("BUWIZ_BRIDGE_URL", "ROX_BRIDGE_URL"),
    ("BUWIZ_BRIDGE_TOKEN", "ROX_BRIDGE_TOKEN"),
    // tests
    ("BUWIZ_STUB_LOG", "ROX_STUB_LOG"),
    // scripts
    ("BUWIZ_APP", "ROX_APP"),
    ("BUWIZ_LOG", "ROX_LOG"),
    ("BUWIZ_BUILD_LOCK", "ROX_BUILD_LOCK"),
    ("BUWIZ_BUILD_HOLD", "ROX_BUILD_HOLD"),
    ("BUWIZ_BOT_BIN", "ROX_BIN"),
    ("BUWIZ_BROKER_BIN", "ROX_BROKER_BIN"),
];

/// The old name of `new`, when it had one.
pub fn old_name(new: &str) -> Option<&'static str> {
    NAMES.iter().find(|(n, _)| *n == new).map(|(_, o)| *o)
}

/// `new`, then its old name when it had one: the names to set for a child
/// that may be an older build, or to remove from one.
pub fn with_old(new: &'static str) -> impl Iterator<Item = &'static str> + Clone {
    std::iter::once(new).chain(old_name(new))
}

/// `new`'s value in the environment, else its old name's (with the
/// warning).
pub fn var_os(new: &str) -> Option<OsString> {
    var_in(new, |k| std::env::var_os(k))
}

/// [`var_os`] as a string; `None` when unset or not Unicode.
pub fn var(new: &str) -> Option<String> {
    var_os(new).and_then(|v| v.into_string().ok())
}

/// Whether `new` (or its old name) is set at all.
pub fn is_set(new: &str) -> bool {
    var_os(new).is_some()
}

/// [`var_os`] over `get` instead of the process environment.
pub fn var_in(new: &str, get: impl Fn(&str) -> Option<OsString>) -> Option<OsString> {
    let v = get(new);
    if v.as_ref().is_some_and(|v| !v.is_empty()) {
        return v;
    }
    // unset, or empty while the old name has a value: the old name's
    let Some(old) = old_name(new) else { return v };
    match get(old) {
        Some(o) if v.is_none() || !o.is_empty() => {
            warn_once(old, new);
            Some(o)
        }
        _ => v,
    }
}

/// Says once per old name that it is old: true when it said it now.
fn warn_once(old: &'static str, new: &str) -> bool {
    static WARNED: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let first = WARNED.lock().map(|mut w| w.get_or_insert_with(HashSet::new).insert(old)).unwrap_or(false);
    if first {
        eprintln!("{old} is an old name; use {new}");
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let m: HashMap<String, OsString> = pairs.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn the_new_name_wins() {
        let get = env(&[("BUWIZ_DATA_DIR", "/new"), ("ROX_DATA_DIR", "/old")]);
        assert_eq!(var_in("BUWIZ_DATA_DIR", get), Some("/new".into()));
        // set but empty still counts as set
        assert_eq!(var_in("BUWIZ_BACKEND", env(&[("BUWIZ_BACKEND", "")])), Some("".into()));
        assert_eq!(var_in("BUWIZ_BACKEND", env(&[("BUWIZ_BACKEND", ""), ("ROX_BACKEND", "")])), Some("".into()));
        // but gives way to an old name with a value
        let get = env(&[("BUWIZ_DATA_DIR", ""), ("ROX_DATA_DIR", "/old")]);
        assert_eq!(var_in("BUWIZ_DATA_DIR", get), Some("/old".into()));
    }

    #[test]
    fn the_old_name_is_still_read_and_warned_about_once() {
        let get = env(&[("GROK_BUNDLE_ID", "dev.example")]);
        assert_eq!(var_in("BUWIZ_BUNDLE_ID", &get), Some("dev.example".into()));
        assert_eq!(var_in("BUWIZ_BUNDLE_ID", &get), Some("dev.example".into()));
        // the reads above warned already
        assert!(!warn_once("GROK_BUNDLE_ID", "BUWIZ_BUNDLE_ID"));
        assert!(warn_once("ROX_TEST_ONLY_NAME", "BUWIZ_TEST_ONLY_NAME"));
        assert!(!warn_once("ROX_TEST_ONLY_NAME", "BUWIZ_TEST_ONLY_NAME"));
    }

    #[test]
    fn neither_name_is_none() {
        assert_eq!(var_in("BUWIZ_PUBLIC_PORT", env(&[("PORT", "1")])), None);
        // a name with no old one reads only itself
        assert_eq!(old_name("BUWIZ_TRUST_FILE"), None);
        assert_eq!(var_in("BUWIZ_TRUST_FILE", env(&[("ROX_TRUST_FILE", "x")])), None);
    }

    #[test]
    fn the_process_environment_is_read() {
        // a name nothing else in this crate reads
        std::env::set_var("ROX_SLOTS", "1");
        std::env::remove_var("BUWIZ_SLOTS");
        assert!(is_set("BUWIZ_SLOTS"));
        assert_eq!(var("BUWIZ_SLOTS").as_deref(), Some("1"));
        std::env::remove_var("ROX_SLOTS");
        assert!(!is_set("BUWIZ_SLOTS"));
    }

    #[test]
    fn with_old_names_both() {
        assert_eq!(with_old("BUWIZ_BROKER_DATA_DIR").collect::<Vec<_>>(), ["BUWIZ_BROKER_DATA_DIR", "GROK_DATA_DIR"]);
        assert_eq!(with_old("BUWIZ_TRUST_FILE").collect::<Vec<_>>(), ["BUWIZ_TRUST_FILE"]);
    }

    #[test]
    fn the_table_has_each_name_once() {
        let mut seen = HashSet::new();
        for (new, old) in NAMES {
            assert!(new.starts_with("BUWIZ_"), "{new}");
            assert!(old.starts_with("ROX_") || old.starts_with("GROK_"), "{old}");
            assert!(seen.insert(*new) && seen.insert(*old), "{new} / {old} twice");
        }
    }

    /// The table is pinned in tests/pins/env_names.json: a change to it is
    /// deliberate (refresh the pin the way test-support/pins.rs says).
    #[test]
    fn the_table_is_the_pinned_one() {
        let rust: Vec<Vec<&str>> = NAMES.iter().map(|(n, o)| vec![*n, *o]).collect();
        crate::pins::check("env_names", &serde_json::json!(rust));
    }
}
