//! Buwiz Bot's environment variables, read by their `BUWIZ_*` names.

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

#[cfg(test)]
mod tests {
    use super::*;

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
