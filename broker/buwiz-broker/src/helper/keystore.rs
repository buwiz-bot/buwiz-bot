//! Where plugin private keys live (spec 4.3; server/src/v1/keystore.mjs).
//!
//! One ed25519 seed per installed plugin. Default: the macOS login
//! Keychain, service `com.buwiz.bot.plugin-key`, account = plugin id,
//! reached only through `/usr/bin/security` (rule R5): that tool already
//! has access to Node's items, so a rebuilt buwiz-broker never prompts. A
//! seed goes to `security -i` on stdin, never in argv, and is never
//! logged. Keys Rox saved under `dev.goldcoders.rox.plugin-key` are moved
//! over once ([`Keystore::move_from_rox`]). `BUWIZ_KEYSTORE=file:<dir>` keeps
//! one 0600 file per plugin instead; the tests use it, or a keychain file
//! of their own.
//!
//! A mini app account's tokens (§20) live beside them: service
//! `com.buwiz.bot.account-token`, account `<plugin>/<account>/<label>`, the
//! token JSON as base64url (so `security -i` takes it bare); a file store
//! keeps `account-tokens/<plugin>~<account>~<label>.json`.

use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use buwiz_protocol::ids::plugin_id_ok;
use buwiz_protocol::keys::SessionKey;

pub const KEYCHAIN_SERVICE: &str = "com.buwiz.bot.plugin-key";
/// Mini app account tokens (§20).
pub const ACCOUNT_SERVICE: &str = "com.buwiz.bot.account-token";
/// Rox's service, before the rename: its keys are moved over once.
pub const OLD_KEYCHAIN_SERVICE: &str = "dev.goldcoders.rox.plugin-key";
const SECURITY: &str = "/usr/bin/security";

/// The Keychain items under `service`, in `keychain`: a keychain file (no
/// spaces), or the user's search list when `None` (tests give their own).
struct Items<'a> {
    service: &'a str,
    keychain: Option<&'a str>,
}

const ITEMS: Items<'static> = Items { service: KEYCHAIN_SERVICE, keychain: None };
const ACCOUNT_ITEMS: Items<'static> = Items { service: ACCOUNT_SERVICE, keychain: None };

impl Items<'_> {
    fn put(&self, id: &str, seed: &str) -> Result<(), String> {
        // ids and seeds hold no spaces or quotes (grammar, base64url), so
        // the line needs no quoting
        let kc = self.keychain.map(|k| format!(" {k}")).unwrap_or_default();
        security_stdin(&format!("add-generic-password -U -s {} -a {id} -w {seed}{kc}", self.service))
    }

    fn get(&self, id: &str) -> Option<String> {
        let mut args = vec!["find-generic-password", "-s", self.service, "-a", id, "-w"];
        args.extend(self.keychain);
        security(&args).map(|out| out.trim().to_owned())
    }

    /// Whether the item went (or was not there).
    fn remove(&self, id: &str) -> bool {
        let mut args = vec!["delete-generic-password", "-s", self.service, "-a", id];
        args.extend(self.keychain);
        security(&args).is_some() || self.get(id).is_none()
    }

    /// The keys of `ids` moved here from `old`: each read, written here
    /// unless one is here already (the newer), and deleted from `old`.
    /// → how many moved; the first failure stops it.
    fn move_from(&self, old: &Items<'_>, ids: &[String]) -> Result<usize, String> {
        let mut moved = 0;
        for id in ids {
            let Some(seed) = old.get(id) else { continue };
            if self.get(id).is_none() {
                check_seed(&seed).map_err(|e| format!("{id}: {e}"))?;
                self.put(id, &seed)?;
                if self.get(id).as_deref() != Some(seed.as_str()) {
                    return Err(format!("{id}: the copy under {} reads back differently", self.service));
                }
                moved += 1;
            }
            if !old.remove(id) {
                return Err(format!("{id}: not deleted from {}", old.service));
            }
        }
        Ok(moved)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Keystore {
    File(PathBuf),
    Keychain,
}

fn check_id(id: &str) -> Result<(), String> {
    if plugin_id_ok(id) {
        Ok(())
    } else {
        Err(format!("not a plugin id: {}", id.chars().take(80).collect::<String>()))
    }
}

/// An account label: `^[A-Za-z0-9][A-Za-z0-9._@+-]{0,59}$` (no spaces or
/// quotes: it is part of a Keychain account name on a `security -i` line).
pub fn label_ok(s: &str) -> bool {
    let mut cs = s.chars();
    s.len() <= 60 && cs.next().is_some_and(|c| c.is_ascii_alphanumeric()) && cs.all(|c| c.is_ascii_alphanumeric() || "._@+-".contains(c))
}

/// `<plugin>/<account>/<label>`, each part of its grammar.
fn check_ref(r: &str) -> Result<(), String> {
    let parts: Vec<&str> = r.split('/').collect();
    match parts.as_slice() {
        [p, a, l] if plugin_id_ok(p) && crate::v1::package::account_id_ok(a) && label_ok(l) => Ok(()),
        _ => Err(format!("not an account token reference: {}", r.chars().take(120).collect::<String>())),
    }
}

fn check_seed(seed: &str) -> Result<(), String> {
    SessionKey::from_seed(seed).map(|_| ()).map_err(|_| "a plugin key seed is 32 bytes of base64url".to_owned())
}

impl Keystore {
    /// The keystore `BUWIZ_KEYSTORE` names (`file:<dir>` or `keychain`); the
    /// Keychain when it is empty.
    pub fn from_spec(spec: &str) -> Result<Keystore, String> {
        if let Some(dir) = spec.strip_prefix("file:") {
            return Ok(Keystore::File(PathBuf::from(dir)));
        }
        if !spec.is_empty() && spec != "keychain" {
            return Err(format!("BUWIZ_KEYSTORE must be file:<dir> or keychain, not {}", spec.chars().take(40).collect::<String>()));
        }
        Ok(Keystore::Keychain)
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Keystore::File(_) => "file",
            Keystore::Keychain => "keychain",
        }
    }

    pub fn put(&self, id: &str, seed: &str) -> Result<(), String> {
        check_id(id)?;
        check_seed(seed)?;
        match self {
            Keystore::File(dir) => {
                if !dir.exists() {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
                }
                let path = dir.join(format!("{id}.key"));
                let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path).map_err(|e| e.to_string())?;
                f.write_all(seed.as_bytes()).map_err(|e| e.to_string())?;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())
            }
            Keystore::Keychain => ITEMS.put(id, seed),
        }
    }

    /// The seed, or `None` when there is none (or the Keychain refused).
    pub fn get(&self, id: &str) -> Result<Option<String>, String> {
        check_id(id)?;
        Ok(match self {
            Keystore::File(dir) => std::fs::read_to_string(dir.join(format!("{id}.key"))).ok().map(|s| s.trim().to_owned()),
            Keystore::Keychain => ITEMS.get(id),
        })
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        check_id(id)?;
        match self {
            Keystore::File(dir) => match std::fs::remove_file(dir.join(format!("{id}.key"))) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            },
            Keystore::Keychain => {
                ITEMS.remove(id);
                Ok(())
            }
        }
    }

    fn token_file(dir: &std::path::Path, r: &str) -> PathBuf {
        dir.join("account-tokens").join(format!("{}.json", r.replace('/', "~")))
    }

    /// Stores an account's tokens under `r` (`<plugin>/<account>/<label>`).
    pub fn put_token(&self, r: &str, tokens: &serde_json::Value) -> Result<(), String> {
        check_ref(r)?;
        let text = tokens.to_string();
        match self {
            Keystore::File(dir) => {
                let sub = dir.join("account-tokens");
                std::fs::create_dir_all(&sub).map_err(|e| e.to_string())?;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
                let _ = std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o700));
                let path = Self::token_file(dir, r);
                let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path).map_err(|e| e.to_string())?;
                f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())
            }
            Keystore::Keychain => ACCOUNT_ITEMS.put(r, &buwiz_protocol::keys::b64(text.as_bytes())),
        }
    }

    /// An account's tokens, or `None` when there are none.
    pub fn get_token(&self, r: &str) -> Result<Option<serde_json::Value>, String> {
        check_ref(r)?;
        let text = match self {
            Keystore::File(dir) => std::fs::read_to_string(Self::token_file(dir, r)).ok(),
            Keystore::Keychain => ACCOUNT_ITEMS.get(r).and_then(|b| buwiz_protocol::keys::unb64(&b).ok()).and_then(|b| String::from_utf8(b).ok()),
        };
        Ok(text.and_then(|t| serde_json::from_str(&t).ok()))
    }

    pub fn remove_token(&self, r: &str) -> Result<(), String> {
        check_ref(r)?;
        match self {
            Keystore::File(dir) => match std::fs::remove_file(Self::token_file(dir, r)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            },
            Keystore::Keychain => {
                ACCOUNT_ITEMS.remove(r);
                Ok(())
            }
        }
    }

    /// Moves the keys of `ids` (the installed plugins) from Rox's service
    /// to ours, once: `marker` (a file beside the database, which the Node
    /// broker shares) says it is done, and a failure writes none, so the
    /// next start tries again. A file store has nothing to move.
    pub fn move_from_rox(&self, ids: &[String], marker: &std::path::Path) -> Result<usize, String> {
        if *self != Keystore::Keychain || marker.exists() {
            return Ok(0);
        }
        let ids: Vec<String> = ids.iter().filter(|id| plugin_id_ok(id)).cloned().collect();
        let n = ITEMS.move_from(&Items { service: OLD_KEYCHAIN_SERVICE, keychain: None }, &ids)?;
        std::fs::write(marker, format!("{{\"moved\":{n}}}\n")).map_err(|e| e.to_string())?;
        Ok(n)
    }
}

/// [`Keystore::move_from_rox`]'s marker, beside the plugins' database.
pub const MOVED_MARKER: &str = ".plugin-keys-moved-from-rox";

/// `security -i` with one command on stdin (it has no quit command: the
/// session ends with stdin). Fails on a non-zero exit or any stderr.
fn security_stdin(line: &str) -> Result<(), String> {
    let mut child = Command::new(SECURITY)
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(format!("{line}\n").as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    if out.status.success() && err.is_empty() {
        Ok(())
    } else if err.is_empty() {
        Err(format!("security: exit {}", out.status.code().unwrap_or(-1)))
    } else {
        Err(format!("security: {}", err.chars().take(200).collect::<String>()))
    }
}

/// `security <args>`'s stdout, or `None` on failure or after 10 s.
pub(crate) fn security(args: &[&str]) -> Option<String> {
    let mut child = Command::new(SECURITY).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                use std::io::Read as _;
                child.stdout.take()?.read_to_string(&mut out).ok()?;
                return status.success().then_some(out);
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "dev.example.fixture";

    fn seed() -> String {
        buwiz_protocol::keys::b64(&[9; 32])
    }

    #[test]
    fn the_spec_picks_the_store() {
        assert_eq!(Keystore::from_spec("").unwrap(), Keystore::Keychain);
        assert_eq!(Keystore::from_spec("keychain").unwrap(), Keystore::Keychain);
        assert_eq!(Keystore::from_spec("file:/k").unwrap(), Keystore::File("/k".into()));
        assert!(Keystore::from_spec("vault").unwrap_err().contains("file:<dir> or keychain"));
    }

    #[test]
    fn a_file_store_keeps_one_private_file_per_plugin() {
        let dir = std::env::temp_dir().join(format!("rox-keystore-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ks = Keystore::File(dir.clone());
        assert_eq!(ks.get(ID).unwrap(), None);
        ks.put(ID, &seed()).unwrap();
        assert_eq!(ks.get(ID).unwrap(), Some(seed()));
        let mode = std::fs::metadata(dir.join(format!("{ID}.key"))).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        ks.remove(ID).unwrap();
        ks.remove(ID).unwrap();
        assert_eq!(ks.get(ID).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A keychain file of the test's own (never the login keychain, never
    /// on the search list) and test service names.
    #[cfg(target_os = "macos")]
    #[test]
    fn rox_keys_move_to_the_new_service_once() {
        let dir = std::env::temp_dir().join(format!("rox-keystore-kc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let kc = dir.join("test.keychain-db");
        let kc = kc.to_str().unwrap();
        assert!(!kc.contains(' '));
        let run = |args: &[&str]| assert!(Command::new(SECURITY).args(args).stdout(Stdio::null()).status().unwrap().success(), "security {args:?}");
        run(&["create-keychain", "-p", "test", kc]);
        run(&["unlock-keychain", "-p", "test", kc]);
        let (new, old) = (format!("com.buwiz.bot.test-plugin-key.{}", std::process::id()), format!("dev.goldcoders.rox.test-plugin-key.{}", std::process::id()));
        let (new, old) = (Items { service: &new, keychain: Some(kc) }, Items { service: &old, keychain: Some(kc) });
        const OTHER: &str = "dev.example.other";
        const NEWER: &str = "dev.example.newer";
        old.put(ID, &seed()).unwrap();
        old.put(OTHER, &seed()).unwrap();
        old.put(NEWER, &seed()).unwrap();
        let newer = buwiz_protocol::keys::b64(&[7; 32]);
        new.put(NEWER, &newer).unwrap();
        let ids = [ID.to_owned(), NEWER.to_owned(), "dev.example.none".to_owned()];
        assert_eq!(new.move_from(&old, &ids).unwrap(), 1);
        assert_eq!(new.get(ID), Some(seed()));
        assert_eq!(new.get(NEWER), Some(newer), "a key under the new service already is the newer one");
        assert_eq!((old.get(ID), old.get(NEWER)), (None, None), "deleted from the old service");
        assert_eq!(old.get(OTHER), Some(seed()), "only the given plugins are touched");
        assert_eq!(new.move_from(&old, &ids).unwrap(), 0, "nothing left to move");
        run(&["delete-keychain", kc]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_store_keeps_account_tokens_private_under_their_reference() {
        let dir = std::env::temp_dir().join(format!("rox-keystore-tokens-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ks = Keystore::File(dir.clone());
        let r = "app.buwiz.fixture/google/u@example.com";
        assert_eq!(ks.get_token(r).unwrap(), None);
        let t = serde_json::json!({"access_token": "at", "refresh_token": "rt", "expires_at": 10});
        ks.put_token(r, &t).unwrap();
        assert_eq!(ks.get_token(r).unwrap(), Some(t.clone()));
        let file = dir.join("account-tokens/app.buwiz.fixture~google~u@example.com.json");
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        ks.remove_token(r).unwrap();
        ks.remove_token(r).unwrap();
        assert_eq!(ks.get_token(r).unwrap(), None);
        for bad in ["app.buwiz.fixture/google", "app.buwiz.fixture/Google/default", "app.buwiz.fixture/google/a b", "app.buwiz.fixture/google/../x", "x/google/default"] {
            assert!(ks.put_token(bad, &t).unwrap_err().starts_with("not an account token reference"), "{bad}");
        }
        assert!(label_ok("default") && label_ok("u+1@example.com") && !label_ok(".x") && !label_ok("") && !label_ok(&"a".repeat(61)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The account service's items in a keychain file of the test's own: a
    /// token blob goes through `security -i` bare and reads back.
    #[cfg(target_os = "macos")]
    #[test]
    fn account_tokens_round_trip_through_a_test_keychain() {
        let dir = std::env::temp_dir().join(format!("rox-keystore-acct-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let kc = dir.join("test.keychain-db");
        let kc = kc.to_str().unwrap();
        let run = |args: &[&str]| assert!(Command::new(SECURITY).args(args).stdout(Stdio::null()).status().unwrap().success(), "security {args:?}");
        run(&["create-keychain", "-p", "test", kc]);
        run(&["unlock-keychain", "-p", "test", kc]);
        let service = format!("com.buwiz.bot.test-account-token.{}", std::process::id());
        let items = Items { service: &service, keychain: Some(kc) };
        let r = "app.buwiz.fixture/google/default";
        let blob = buwiz_protocol::keys::b64(serde_json::json!({"access_token": "a b \"c\"", "expires_at": 1}).to_string().as_bytes());
        items.put(r, &blob).unwrap();
        assert_eq!(items.get(r), Some(blob));
        assert!(items.remove(r));
        assert_eq!(items.get(r), None);
        run(&["delete-keychain", kc]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_store_moves_nothing_and_the_marker_stops_a_second_move() {
        let dir = std::env::temp_dir().join(format!("rox-keystore-marker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join(MOVED_MARKER);
        assert_eq!(Keystore::File(dir.clone()).move_from_rox(&[ID.to_owned()], &marker).unwrap(), 0);
        assert!(!marker.exists(), "a file store writes no marker");
        std::fs::write(&marker, "{}").unwrap();
        // with the marker the Keychain is not even asked
        assert_eq!(Keystore::Keychain.move_from_rox(&[ID.to_owned()], &marker).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_ids_and_seeds_are_refused_before_any_store() {
        let ks = Keystore::File(std::env::temp_dir().join("rox-keystore-never"));
        assert!(ks.put("Not An Id", &seed()).unwrap_err().starts_with("not a plugin id"));
        assert!(ks.get("x").is_err());
        assert!(ks.put(ID, "short").unwrap_err().contains("32 bytes"));
        assert!(!std::env::temp_dir().join("rox-keystore-never").exists());
    }
}
