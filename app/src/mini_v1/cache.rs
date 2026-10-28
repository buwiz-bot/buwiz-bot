//! The compiled-module cache and prewarm (spec 14.6, "Host caching and
//! prewarm"): a v1 app's checked and compiled module, kept in memory and
//! on disk under `<plugins dir>/<plugin id>/cache/` (0600, the folder
//! 0700), so an open skips the source fetch and the compile.
//!
//! An entry's key is the plugin id, the app id, the app's **signed
//! manifest fingerprint** and this Rox build (the executable's size and
//! modification time: it carries the nsh compiler, the import table and
//! the bridge shim). A changed fingerprint or a new Rox is a miss. Each
//! file records its key and the SHA-256 of its module; a file whose
//! record does not match is a miss and is removed. Entries of
//! fingerprints no longer in the installed manifest are evicted when the
//! plugin list changes; uninstall deletes the plugin's folder, cache
//! included (13.6).
//!
//! `ROX_MINI_CACHE=0` turns the cache off (to measure the cold path).

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde_json::{Value, json};

use super::keys;

/// Whether the cache is on (`ROX_MINI_CACHE=0` turns it off).
pub fn enabled() -> bool {
    std::env::var("ROX_MINI_CACHE").map_or(true, |v| v != "0")
}

/// This Rox build: what compiled the module (nsh's compiler, the import
/// table, the shim) is part of every key.
pub fn build_id() -> String {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        let exe = std::env::current_exe().ok().and_then(|p| std::fs::metadata(p).ok());
        let stamp = exe
            .map(|m| {
                let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
                format!("{}:{t}", m.len())
            })
            .unwrap_or_default();
        let shim = keys::sha256_hex(super::load::SHIM.as_bytes());
        keys::sha256_hex(format!("{}\n{stamp}\n{shim}", env!("CARGO_PKG_VERSION")).as_bytes())[..16].to_owned()
    })
    .clone()
}

/// An entry's key.
pub fn key(plugin: &str, app: &str, fingerprint: &str, build: &str) -> String {
    keys::sha256_hex(format!("{plugin}\n{app}\n{fingerprint}\n{build}").as_bytes())
}

/// The cache folder of `plugin`.
pub fn dir(root: &Path, plugin: &str) -> PathBuf {
    root.join(plugin).join("cache")
}

fn file(root: &Path, plugin: &str, app: &str) -> PathBuf {
    dir(root, plugin).join(format!("{app}.json"))
}

static MEM: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// The compiled module of `app` of `plugin` at `fingerprint`, if cached:
/// memory first, then disk (a disk hit is kept in memory).
pub fn get(root: &Path, plugin: &str, app: &str, fingerprint: &str, build: &str) -> Option<String> {
    let k = key(plugin, app, fingerprint, build);
    if let Some(js) = MEM.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&k).cloned()) {
        return Some(js);
    }
    let path = file(root, plugin, app);
    let v: Value = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
    let js = v.get("js").and_then(Value::as_str)?;
    let ok = v.get("key").and_then(Value::as_str) == Some(k.as_str())
        && v.get("sha256").and_then(Value::as_str) == Some(keys::sha256_hex(js.as_bytes()).as_str());
    if !ok {
        // another fingerprint or build, or a damaged file: a miss
        if v.get("key").and_then(Value::as_str) != Some(k.as_str()) {
            return None;
        }
        let _ = std::fs::remove_file(&path);
        return None;
    }
    MEM.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(k, js.to_owned());
    Some(js.to_owned())
}

/// Keeps `js` as `app`'s module at `fingerprint` (one file per app: a new
/// fingerprint replaces the old entry).
pub fn put(root: &Path, plugin: &str, app: &str, fingerprint: &str, build: &str, js: &str) {
    let k = key(plugin, app, fingerprint, build);
    MEM.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(k.clone(), js.to_owned());
    let d = dir(root, plugin);
    if std::fs::create_dir_all(&d).is_err() {
        return;
    }
    let body = json!({"key": k, "plugin": plugin, "app": app, "fingerprint": fingerprint, "sha256": keys::sha256_hex(js.as_bytes()), "js": js});
    let tmp = d.join(format!(".{app}.tmp"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700));
    }
    if write_private(&tmp, body.to_string().as_bytes()).is_ok() {
        let _ = std::fs::rename(&tmp, file(root, plugin, app));
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)?.write_all(bytes)
}

/// Removes `plugin`'s entries whose fingerprint is not among `current`
/// (`(app id, fingerprint)` of the installed manifest). How many went.
pub fn evict(root: &Path, plugin: &str, current: &[(String, String)]) -> usize {
    let keep: HashSet<(String, String)> = current.iter().cloned().collect();
    let Ok(entries) = std::fs::read_dir(dir(root, plugin)) else { return 0 };
    let mut n = 0;
    for e in entries.flatten() {
        let path = e.path();
        let v: Option<Value> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let app = v.as_ref().and_then(|v| v.get("app")).and_then(Value::as_str).unwrap_or_default().to_owned();
        let fp = v.as_ref().and_then(|v| v.get("fingerprint")).and_then(Value::as_str).unwrap_or_default().to_owned();
        if !keep.contains(&(app, fp)) && std::fs::remove_file(&path).is_ok() {
            n += 1;
        }
    }
    // and from memory: keys of this plugin not current are unreachable
    // anyway (the key carries the fingerprint), so memory is left as is
    n
}

/// Forgets every in-memory entry (a plugin uninstalled; tests).
pub fn clear_memory() {
    MEM.lock().unwrap_or_else(|e| e.into_inner()).take();
}

/// Prewarm bookkeeping: which (plugin, generation) were warmed, so each
/// generation is warmed once.
#[derive(Default)]
pub struct Prewarm {
    done: HashSet<(String, u64)>,
}

impl Prewarm {
    /// Whether `plugin` at `generation` is due a prewarm (and marks it).
    pub fn due(&mut self, plugin: &str, generation: u64) -> bool {
        self.done.insert((plugin.to_owned(), generation))
    }
}

/// The apps of a listed plugin to prewarm: every app whose placement
/// includes `inline` (answer cards), with its fingerprint.
pub fn warm_apps(apps: &[Value]) -> Vec<(String, String)> {
    apps.iter()
        .filter(|a| a.get("placement").and_then(Value::as_array).is_some_and(|p| p.iter().any(|x| x == "inline")))
        .filter_map(|a| Some((a.get("id")?.as_str()?.to_owned(), a.get("fingerprint")?.as_str()?.to_owned())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rox-cache-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    const P: &str = "dev.goldcoders.cachefx";

    #[test]
    fn perf_01_a_hit_skips_the_compile_and_a_new_fingerprint_or_build_is_a_miss() {
        let r = root("hit");
        clear_memory();
        assert_eq!(get(&r, P, "deadlines", "sha256:aa", "b1"), None);
        put(&r, P, "deadlines", "sha256:aa", "b1", "compiled js");
        assert_eq!(get(&r, P, "deadlines", "sha256:aa", "b1").as_deref(), Some("compiled js"));
        // from disk too (a fresh process)
        clear_memory();
        assert_eq!(get(&r, P, "deadlines", "sha256:aa", "b1").as_deref(), Some("compiled js"));
        assert_eq!(get(&r, P, "deadlines", "sha256:bb", "b1"), None, "a new fingerprint");
        assert_eq!(get(&r, P, "deadlines", "sha256:aa", "b2"), None, "a new Rox build");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(file(&r, P, "deadlines")).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            assert_eq!(std::fs::metadata(dir(&r, P)).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn perf_01_a_damaged_entry_is_a_miss_and_is_removed() {
        let r = root("damaged");
        clear_memory();
        put(&r, P, "card", "sha256:aa", "b1", "good js");
        clear_memory();
        let path = file(&r, P, "card");
        let text = std::fs::read_to_string(&path).unwrap().replace("good js", "evil js");
        std::fs::write(&path, text).unwrap();
        assert_eq!(get(&r, P, "card", "sha256:aa", "b1"), None);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn perf_02_entries_of_fingerprints_no_longer_installed_are_evicted_and_uninstall_removes_the_cache() {
        let r = root("evict");
        clear_memory();
        put(&r, P, "a", "sha256:old", "b1", "a old");
        put(&r, P, "b", "sha256:keep", "b1", "b");
        assert_eq!(evict(&r, P, &[("a".into(), "sha256:new".into()), ("b".into(), "sha256:keep".into())]), 1);
        clear_memory();
        assert_eq!(get(&r, P, "a", "sha256:old", "b1"), None);
        assert_eq!(get(&r, P, "b", "sha256:keep", "b1").as_deref(), Some("b"));
        // uninstall (13.6) deletes the plugin's folder: its cache with it
        super::super::storage::delete_plugin(&r, P).unwrap();
        clear_memory();
        assert!(!dir(&r, P).exists());
        assert_eq!(get(&r, P, "b", "sha256:keep", "b1"), None);
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn perf_03_prewarm_runs_once_per_generation_for_inline_apps() {
        let mut w = Prewarm::default();
        assert!(w.due(P, 3));
        assert!(!w.due(P, 3));
        assert!(w.due(P, 4), "a restart is a new generation");
        assert!(w.due("dev.goldcoders.other", 3));
        let apps = vec![
            json!({"id": "forms", "placement": ["full"], "fingerprint": "sha256:f"}),
            json!({"id": "deadlines", "placement": ["inline", "full"], "fingerprint": "sha256:d"}),
        ];
        assert_eq!(warm_apps(&apps), [("deadlines".to_owned(), "sha256:d".to_owned())]);
    }
}
