//! Where plugin private keys live (spec 4.3; server/src/v1/keystore.mjs).
//!
//! One ed25519 seed per installed plugin. Default: the macOS login
//! Keychain, service `dev.goldcoders.rox.plugin-key`, account = plugin id,
//! reached only through `/usr/bin/security` (rule R5): that tool already
//! has access to Node's items, so a rebuilt rox-broker never prompts. A
//! seed goes to `security -i` on stdin, never in argv, and is never
//! logged. `ROX_KEYSTORE=file:<dir>` keeps one 0600 file per plugin
//! instead; the tests use it.

use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rox_v1::ids::plugin_id_ok;
use rox_v1::keys::SessionKey;

pub const KEYCHAIN_SERVICE: &str = "dev.goldcoders.rox.plugin-key";
const SECURITY: &str = "/usr/bin/security";

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

fn check_seed(seed: &str) -> Result<(), String> {
    SessionKey::from_seed(seed).map(|_| ()).map_err(|_| "a plugin key seed is 32 bytes of base64url".to_owned())
}

impl Keystore {
    /// The keystore `ROX_KEYSTORE` names (`file:<dir>` or `keychain`); the
    /// Keychain when it is empty.
    pub fn from_spec(spec: &str) -> Result<Keystore, String> {
        if let Some(dir) = spec.strip_prefix("file:") {
            return Ok(Keystore::File(PathBuf::from(dir)));
        }
        if !spec.is_empty() && spec != "keychain" {
            return Err(format!("ROX_KEYSTORE must be file:<dir> or keychain, not {}", spec.chars().take(40).collect::<String>()));
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
            // ids and seeds hold no spaces or quotes (grammar, base64url), so
            // the line needs no quoting
            Keystore::Keychain => security_stdin(&format!("add-generic-password -U -s {KEYCHAIN_SERVICE} -a {id} -w {seed}")),
        }
    }

    /// The seed, or `None` when there is none (or the Keychain refused).
    pub fn get(&self, id: &str) -> Result<Option<String>, String> {
        check_id(id)?;
        Ok(match self {
            Keystore::File(dir) => std::fs::read_to_string(dir.join(format!("{id}.key"))).ok().map(|s| s.trim().to_owned()),
            Keystore::Keychain => security(&["find-generic-password", "-s", KEYCHAIN_SERVICE, "-a", id, "-w"]).map(|out| out.trim().to_owned()),
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
                security(&["delete-generic-password", "-s", KEYCHAIN_SERVICE, "-a", id]);
                Ok(())
            }
        }
    }
}

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
fn security(args: &[&str]) -> Option<String> {
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
        rox_v1::keys::b64(&[9; 32])
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

    #[test]
    fn bad_ids_and_seeds_are_refused_before_any_store() {
        let ks = Keystore::File(std::env::temp_dir().join("rox-keystore-never"));
        assert!(ks.put("Not An Id", &seed()).unwrap_err().starts_with("not a plugin id"));
        assert!(ks.get("x").is_err());
        assert!(ks.put(ID, "short").unwrap_err().contains("32 bytes"));
        assert!(!std::env::temp_dir().join("rox-keystore-never").exists());
    }
}
