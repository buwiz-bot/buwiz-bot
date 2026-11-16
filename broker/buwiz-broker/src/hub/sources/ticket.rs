//! A widget's ticket: the proof its code carries to `POST /w/<name>/<ticket>/call`.
//!
//! `ticket = base64url(HMAC-SHA256(secret, "widget:" + base name))`, `secret` 32 random
//! bytes the broker makes once, in its data directory, readable by its owner only. A
//! widget's ticket never changes (a pop-out bundle that holds it can be cached) and says
//! nothing of who holds it: it lets a widget's own code ask, and what the person allowed
//! that widget is [`super::grants`]'.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use buwiz_protocol::keys::{b64, random};
use buwiz_protocol::proof::{hmac_sha256, same};

use crate::hub::ctx::HubCtx;

const FILE: &str = "widget-ticket-secret";

/// `R<digits>` and no more: a widget's base name.
pub fn is_widget(name: &str) -> bool {
    name.strip_prefix('R').is_some_and(|d| !d.is_empty() && d.len() <= 12 && d.bytes().all(|b| b.is_ascii_digit()))
}

/// The base name of `R37` or `R37@v2`, if it is a widget's.
pub fn base_of(name: &str) -> Option<&str> {
    let base = name.split('@').next().unwrap_or("");
    let version = name.get(base.len()..).unwrap_or("");
    let version_ok = version.is_empty() || version.strip_prefix("@v").is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
    (is_widget(base) && version_ok).then_some(base)
}

fn cache() -> &'static Mutex<HashMap<PathBuf, [u8; 32]>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, [u8; 32]>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn read(path: &Path) -> std::io::Result<Option<[u8; 32]>> {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(<[u8; 32]>::try_from(buf.as_slice()).ok())
}

/// The secret in `<data dir>/widget-ticket-secret` (mode 0600), made on first use. A file of
/// the wrong size is a partial write of another process or damage: it is replaced.
pub fn secret_in(data: &Path) -> Result<[u8; 32], String> {
    let path = data.join(FILE);
    if let Some(s) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(&path) {
        return Ok(*s);
    }
    let found = match read(&path).map_err(|e| format!("{}: {e}", path.display()))? {
        Some(s) => s,
        None => {
            std::fs::create_dir_all(data).map_err(|e| format!("{}: {e}", data.display()))?;
            // written whole under another name, then linked in: a second process finds the
            // file complete or not at all
            let fresh: [u8; 32] = random();
            let tmp = data.join(format!(".{FILE}.{}", buwiz_protocol::keys::random_id()));
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
            f.write_all(&fresh).and_then(|_| f.sync_all()).map_err(|e| format!("{}: {e}", tmp.display()))?;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
            let linked = std::fs::hard_link(&tmp, &path);
            let _ = std::fs::remove_file(&tmp);
            match linked {
                Ok(()) => fresh,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => read(&path).map_err(|e| e.to_string())?.ok_or_else(|| format!("{} is not a 32-byte secret", path.display()))?,
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
    };
    cache().lock().unwrap_or_else(|e| e.into_inner()).insert(path, found);
    Ok(found)
}

/// The ticket of widget `base` (an `R<digits>` name).
pub fn ticket(ctx: &HubCtx, base: &str) -> Result<String, String> {
    Ok(b64(&hmac_sha256(&secret_in(&ctx.data)?, format!("widget:{base}").as_bytes())))
}

/// Whether `given` is widget `base`'s ticket (compared in constant time).
pub fn valid(ctx: &HubCtx, base: &str, given: &str) -> bool {
    is_widget(base) && ticket(ctx, base).is_ok_and(|want| same(given.as_bytes(), want.as_bytes()))
}

/// The path a widget's code calls: `/w/<base>/<ticket>/call`.
pub fn call_path(ctx: &HubCtx, base: &str) -> Result<String, String> {
    Ok(format!("/w/{base}/{}/call", ticket(ctx, base)?))
}
