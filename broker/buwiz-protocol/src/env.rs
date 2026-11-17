//! Buwiz Bot's environment variables, read by their `BUWIZ_*` names.
//!
//! A variable that holds a name of an earlier version is not read, and it is
//! not ignored either: [`refuse_old_names`] stops the program, so a bench
//! that still sets one never runs against the owner's port, data or Keychain.

use std::ffi::OsString;

/// `name`'s value in the environment.
pub fn var_os(name: &str) -> Option<OsString> {
    var_in(name, |k| std::env::var_os(k))
}

/// [`var_os`] as a string; `None` when unset or not Unicode.
pub fn var(name: &str) -> Option<String> {
    var_os(name).and_then(|v| v.into_string().ok())
}

/// Whether `name` is set at all.
pub fn is_set(name: &str) -> bool {
    var_os(name).is_some()
}

/// [`var_os`] over `get` instead of the process environment.
pub fn var_in(name: &str, get: impl Fn(&str) -> Option<OsString>) -> Option<OsString> {
    get(name)
}

/// The names that are not `ROX_*` and were once read.
const OLD_EXACT: [&str; 3] = ["GROK_DATA_DIR", "GROK_ALLOWED_ORIGINS", "GROK_BUNDLE_ID"];

/// Whether `name` is a name of an earlier version: any `ROX_*`, or one of [`OLD_EXACT`].
pub fn is_old_name(name: &str) -> bool {
    name.starts_with("ROX_") || OLD_EXACT.contains(&name)
}

/// The old names among `names`, sorted; the refusal line when there are any.
pub fn old_names_in<I: IntoIterator<Item = OsString>>(names: I) -> Result<(), String> {
    let mut found: Vec<String> = names.into_iter().filter_map(|n| n.into_string().ok()).filter(|n| is_old_name(n)).collect();
    if found.is_empty() {
        return Ok(());
    }
    found.sort();
    Err(format!("these variables are set and no longer read: {}; use the BUWIZ_ name instead and unset them", found.join(", ")))
}

/// [`old_names_in`] over the process environment. Call it first thing in a
/// program, before it opens anything.
pub fn refuse_old_names() -> Result<(), String> {
    old_names_in(std::env::vars_os().map(|(k, _)| k))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn an_old_name_set_is_refused_and_named() {
        let e = old_names_in(names(&["PATH", "ROX_BACKGROUND", "BUWIZ_BACKEND", "GROK_DATA_DIR", "ROX_NO_KEYCHAIN"])).unwrap_err();
        assert_eq!(e, "these variables are set and no longer read: GROK_DATA_DIR, ROX_BACKGROUND, ROX_NO_KEYCHAIN; use the BUWIZ_ name instead and unset them");
        for old in ["GROK_ALLOWED_ORIGINS", "GROK_BUNDLE_ID", "ROX_X"] {
            assert!(old_names_in(names(&[old])).is_err(), "{old}");
        }
    }

    #[test]
    fn no_old_name_starts_and_buwiz_names_are_unaffected() {
        assert_eq!(old_names_in(names(&[])), Ok(()));
        assert_eq!(old_names_in(names(&["PATH", "HOME", "BUWIZ_BACKEND", "BUWIZ_DATA_DIR", "BUWIZ_BROKER_DATA_DIR", "GROK_OTHER", "ROXY", "rox_lower"])), Ok(()));
    }

    #[test]
    fn a_name_reads_only_itself() {
        let get = |k: &str| match k {
            "BUWIZ_DATA_DIR" => Some(OsString::from("/new")),
            "OTHER_DATA_DIR" => Some(OsString::from("/other")),
            _ => None,
        };
        assert_eq!(var_in("BUWIZ_DATA_DIR", get), Some("/new".into()));
        assert_eq!(var_in("BUWIZ_BACKEND", get), None);
        // set but empty still counts as set
        assert_eq!(var_in("BUWIZ_BACKEND", |_| Some(OsString::new())), Some("".into()));
    }

    #[test]
    fn the_process_environment_is_read() {
        // a name nothing else in this crate reads
        std::env::set_var("BUWIZ_TEST_ONLY_NAME", "1");
        assert!(is_set("BUWIZ_TEST_ONLY_NAME"));
        assert_eq!(var("BUWIZ_TEST_ONLY_NAME").as_deref(), Some("1"));
        std::env::remove_var("BUWIZ_TEST_ONLY_NAME");
        assert!(!is_set("BUWIZ_TEST_ONLY_NAME"));
    }
}
