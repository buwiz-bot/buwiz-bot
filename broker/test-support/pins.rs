//! Pinned expected output for the broker's tests.
//!
//! A pin is a file under the including crate's `tests/pins/`: JSON (`<name>.json`)
//! or text (`<name>.txt`), one per test or case, readable in a diff. A test
//! computes what the broker gives and calls `check` or `check_text`, which
//! compares it with the pin and fails with the first difference.
//!
//! The pins were recorded from the old Node broker's own modules, which these
//! tests used to run as the reference. To change one on purpose, after a
//! deliberate change in Rust, run the test with `BUWIZ_REFRESH_PINS=1`. A
//! refresh never turns a difference into a pass: if the output equals the
//! pin the test passes; if it differs (or the pin is missing) the pin is
//! rewritten from the Rust output and the test FAILS, naming the file. Review
//! `git diff`, then run again without the variable. CI never sets it.
//! Included with `#[path]` by each crate's tests.

#![allow(dead_code)]

use std::path::PathBuf;

use serde_json::Value;

fn path(name: &str, ext: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/pins").join(format!("{name}.{ext}"))
}

pub fn refreshing() -> bool {
    std::env::var("BUWIZ_REFRESH_PINS").is_ok_and(|v| !v.is_empty() && v != "0")
}

fn put(name: &str, ext: &str, text: &str) {
    let p = path(name, ext);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn read(name: &str, ext: &str) -> String {
    let p = path(name, ext);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("the pin {} is missing ({e}); BUWIZ_REFRESH_PINS=1 writes it from the Rust output", p.display()))
}

/// Rewrites a JSON pin.
pub fn write(name: &str, v: &Value) {
    put(name, "json", &format!("{}\n", serde_json::to_string_pretty(v).unwrap()));
}

/// Rewrites a text pin.
pub fn write_text(name: &str, s: &str) {
    put(name, "txt", s);
}

/// The pinned JSON.
pub fn load(name: &str) -> Value {
    serde_json::from_str(&read(name, "json")).unwrap_or_else(|e| panic!("the pin {name} is not JSON: {e}"))
}

/// The pinned text.
pub fn load_text(name: &str) -> String {
    read(name, "txt")
}

/// Where two JSON values first differ, as a path and the two sides.
fn first_difference(path: &str, ours: &Value, pinned: &Value) -> Option<String> {
    match (ours, pinned) {
        (Value::Array(a), Value::Array(b)) => {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                if let Some(d) = first_difference(&format!("{path}[{i}]"), x, y) {
                    return Some(d);
                }
            }
            (a.len() != b.len()).then(|| format!("{path}: {} items, the pin has {}", a.len(), b.len()))
        }
        (Value::Object(a), Value::Object(b)) => {
            for (k, x) in a {
                match b.get(k) {
                    Some(y) => {
                        if let Some(d) = first_difference(&format!("{path}.{k}"), x, y) {
                            return Some(d);
                        }
                    }
                    None => return Some(format!("{path}.{k}: not in the pin")),
                }
            }
            b.keys().find(|k| !a.contains_key(*k)).map(|k| format!("{path}.{k}: only in the pin"))
        }
        _ => (ours != pinned).then(|| format!("{path}:\n  ours:   {ours}\n  pinned: {pinned}")),
    }
}

/// Fails a refresh that had to rewrite the pin `name` (`ext`: json or txt).
pub fn rewritten(name: &str, ext: &str) -> ! {
    panic!("the pin tests/pins/{name}.{ext} differed (or was missing) and was rewritten from the Rust output: review `git diff`, then run again without BUWIZ_REFRESH_PINS");
}

/// Compares `ours` with the JSON pin `name`.
pub fn check(name: &str, ours: &Value) {
    let pinned = std::fs::read_to_string(path(name, "json")).ok().map(|t| serde_json::from_str::<Value>(&t).unwrap_or_else(|e| panic!("the pin {name} is not JSON: {e}")));
    if refreshing() {
        if pinned.as_ref().is_some_and(|p| first_difference("$", ours, p).is_none()) {
            return;
        }
        write(name, ours);
        rewritten(name, "json");
    }
    let pinned = pinned.unwrap_or_else(|| load(name));
    if let Some(d) = first_difference("$", ours, &pinned) {
        panic!("differs from the pin tests/pins/{name}.json at {d}\n(BUWIZ_REFRESH_PINS=1 rewrites the pin when the change is deliberate)");
    }
}

/// Compares `ours` with the text pin `name`.
pub fn check_text(name: &str, ours: &str) {
    if refreshing() {
        if std::fs::read_to_string(path(name, "txt")).is_ok_and(|p| p == ours) {
            return;
        }
        write_text(name, ours);
        rewritten(name, "txt");
    }
    let pinned = load_text(name);
    if ours != pinned {
        let line = ours.lines().zip(pinned.lines()).position(|(a, b)| a != b).unwrap_or_else(|| ours.lines().count().min(pinned.lines().count()));
        panic!(
            "differs from the pin tests/pins/{name}.txt at line {}:\n  ours:   {:?}\n  pinned: {:?}\n(BUWIZ_REFRESH_PINS=1 rewrites the pin when the change is deliberate)",
            line + 1,
            ours.lines().nth(line),
            pinned.lines().nth(line)
        );
    }
}

/// Fails when `tests/pins/<dir>` holds a JSON pin starting with `prefix` that
/// is not in `read` (names without the extension): a leftover nothing compares.
pub fn assert_all_read(dir: &str, prefix: &str, read: &[String]) {
    let folder = path(&format!("{dir}/x"), "json").parent().unwrap().to_owned();
    let mut left: Vec<String> = std::fs::read_dir(&folder)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|f| f.strip_suffix(".json").map(str::to_owned))
        .filter(|n| n.starts_with(prefix) && !read.contains(&format!("{dir}/{n}")))
        .collect();
    left.sort();
    assert!(left.is_empty(), "tests/pins/{dir} holds pins no test compared: {}", left.join(", "));
}
