//! Each v1 app's private key-value store (spec 11, design §3), kept by
//! Rox at `<plugins dir>/<plugin id>/apps/<app id>/storage.json`. The app
//! names keys, never a path: the directory comes from the open's identity,
//! which lives in Rust, so two apps cannot reach each other's keys.
//!
//! Every operation reads and writes the file under one process-wide lock
//! (two opens of an app share its store), writing to a temporary file and
//! renaming it over, so a crash leaves the old or the new store.
//!
//! The engine's own storage (`nsh/storage`, which v1 apps cannot import)
//! gets a backend of its own per app too ([`engine_storage`]), so even a
//! slip never shares v0's `default/storage.json`.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use super::RoxError;

pub const MAX_KEY: usize = 128;
/// 64 KiB.
pub const MAX_VALUE: usize = 64 * 1024;
pub const MAX_KEYS: usize = 1000;

/// One app's store.
#[derive(Debug, Clone)]
pub struct AppStorage {
    file: PathBuf,
    quota: u64,
}

/// Serializes every store's read-modify-write.
static LOCK: Mutex<()> = Mutex::new(());

/// `<root>/<plugin>/apps/<app>`: an app's directory under the plugins
/// directory `root` (ids are checked by the grammar, so no `..`).
pub fn app_dir(root: &Path, plugin: &str, app: &str) -> PathBuf {
    root.join(plugin).join("apps").join(app)
}

impl AppStorage {
    /// The store of `app` of `plugin` under `root` with `quota` bytes.
    pub fn new(root: &Path, plugin: &str, app: &str, quota: u64) -> AppStorage {
        AppStorage { file: app_dir(root, plugin, app).join("storage.json"), quota }
    }

    pub fn get(&self, key: &str) -> Result<Option<String>, RoxError> {
        check_key(key)?;
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self.read()?.remove(key))
    }

    pub fn set(&self, key: &str, value: &str) -> Result<(), RoxError> {
        check_key(key)?;
        if value.len() > MAX_VALUE {
            return Err(RoxError::new("quota_exceeded", "a stored value is at most 64 KiB"));
        }
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = self.read()?;
        if !all.contains_key(key) && all.len() >= MAX_KEYS {
            return Err(RoxError::new("quota_exceeded", "the store holds at most 1000 keys"));
        }
        all.insert(key.to_owned(), value.to_owned());
        if used(&all) > self.quota {
            return Err(RoxError::new("quota_exceeded", format!("the store's quota is {} bytes", self.quota)));
        }
        self.write(&all)
    }

    pub fn remove(&self, key: &str) -> Result<(), RoxError> {
        check_key(key)?;
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = self.read()?;
        if all.remove(key).is_some() {
            self.write(&all)?;
        }
        Ok(())
    }

    pub fn keys(&self) -> Result<Vec<String>, RoxError> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self.read()?.into_keys().take(MAX_KEYS).collect())
    }

    fn read(&self) -> Result<BTreeMap<String, String>, RoxError> {
        match std::fs::read(&self.file) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| RoxError::new("internal", format!("the app's store is unreadable: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(RoxError::new("internal", format!("the app's store is unreadable: {e}"))),
        }
    }

    fn write(&self, all: &BTreeMap<String, String>) -> Result<(), RoxError> {
        let fail = |e: std::io::Error| RoxError::new("internal", format!("the app's store can't be written: {e}"));
        let dir = self.file.parent().expect("the store's file has a directory");
        make_private_dir(dir).map_err(fail)?;
        let tmp = self.file.with_extension("json.tmp");
        let text = serde_json::to_vec(all).expect("strings serialize");
        std::fs::write(&tmp, text).map_err(fail)?;
        std::fs::rename(&tmp, &self.file).map_err(fail)
    }
}

/// UTF-8 bytes of the keys and values.
fn used(all: &BTreeMap<String, String>) -> u64 {
    all.iter().map(|(k, v)| (k.len() + v.len()) as u64).sum()
}

/// `^[A-Za-z0-9._:-]+$`, at most 128 chars.
fn check_key(key: &str) -> Result<(), RoxError> {
    let ok = !key.is_empty()
        && key.len() <= MAX_KEY
        && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'));
    if ok { Ok(()) } else { Err(RoxError::new("invalid_args", "a key is 1 to 128 of A-Z a-z 0-9 . _ : -")) }
}

/// Makes `dir` (and its parents), readable by this account only.
fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// The engine storage of one app (`AppOptions::storage`): nsh's file
/// backend in `<app dir>/engine/storage.json`, apart from the bridge's
/// store and from every other app.
pub fn engine_storage(root: &Path, plugin: &str, app: &str) -> Arc<dyn nsh_engine::StorageBackend> {
    nsh_engine::FileStorage::open(app_dir(root, plugin, app).join("engine"))
}

/// Deletes every app store of `plugin` (uninstall, spec 13.6 step 5).
pub fn delete_plugin(root: &Path, plugin: &str) -> std::io::Result<()> {
    if !super::perms::plugin_id_ok(plugin) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a plugin id"));
    }
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    match std::fs::remove_dir_all(root.join(plugin)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

#[cfg(test)]
pub(crate) fn scratch_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rox-mini-v1-{name}-{}", super::keys::random_id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brg_16_storage_limits_quota_and_isolation() {
        let root = scratch_root("storage");
        let a = AppStorage::new(&root, "dev.goldcoders.fixture", "board", 300);
        a.set("k.1:x-y_z", "v").unwrap();
        assert_eq!(a.get("k.1:x-y_z").unwrap().as_deref(), Some("v"));
        assert_eq!(a.get("missing").unwrap(), None);
        // a key of 128 chars is the limit; 129 and bad characters are refused
        a.set(&"k".repeat(128), "").unwrap();
        a.remove(&"k".repeat(128)).unwrap();
        for bad in ["k".repeat(129), String::new(), "a/b".to_owned(), "../x".to_owned(), "a b".to_owned()] {
            assert_eq!(a.set(&bad, "v").unwrap_err().code, "invalid_args", "{bad:?}");
        }
        // a value over 64 KiB is refused, at any quota
        let big = AppStorage::new(&root, "dev.goldcoders.fixture", "big", 10 << 20);
        big.set("v", &"x".repeat(MAX_VALUE)).unwrap();
        assert_eq!(big.set("v", &"x".repeat(MAX_VALUE + 1)).unwrap_err().code, "quota_exceeded");
        // the quota counts keys and values; a refused set keeps the store
        assert_eq!(a.set("fill", &"x".repeat(300)).unwrap_err().code, "quota_exceeded");
        assert_eq!(a.keys().unwrap(), ["k.1:x-y_z"]);
        // two apps (and two plugins' apps of one id) don't see each other's keys
        let b = AppStorage::new(&root, "dev.goldcoders.fixture", "card", 300);
        let c = AppStorage::new(&root, "dev.goldcoders.other", "board", 300);
        assert_eq!(b.get("k.1:x-y_z").unwrap(), None);
        assert_eq!(c.get("k.1:x-y_z").unwrap(), None);
        assert!(b.keys().unwrap().is_empty() && c.keys().unwrap().is_empty());
        assert!(root.join("dev.goldcoders.fixture/apps/board/storage.json").is_file());
        // uninstall deletes the plugin's stores only
        delete_plugin(&root, "dev.goldcoders.fixture").unwrap();
        assert!(a.keys().unwrap().is_empty());
        c.set("x", "1").unwrap();
        assert!(delete_plugin(&root, "..").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn brg_16_a_store_lists_at_most_1000_keys() {
        let root = scratch_root("keys");
        let s = AppStorage::new(&root, "dev.goldcoders.fixture", "board", 1 << 20);
        let all: BTreeMap<String, String> = (0..MAX_KEYS).map(|i| (format!("k{i}"), String::new())).collect();
        s.write(&all).unwrap();
        assert_eq!(s.set("one-more", "").unwrap_err().code, "quota_exceeded");
        s.set("k5", "replace").unwrap();
        assert_eq!(s.keys().unwrap().len(), MAX_KEYS);
        let _ = std::fs::remove_dir_all(&root);
    }
}
