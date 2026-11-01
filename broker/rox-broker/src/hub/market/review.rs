//! A plain plugin's review before it installs (extension format 2.1 §2.2,
//! D2): `POST /plugins/inspect` fetches the plugin into a staging folder
//! (`plugins/.staging/<token>/`) and answers what it would add; only
//! `POST /plugins/install {staging, approve: true}` moves it into place.
//! A staging folder lasts 30 minutes; the next inspect deletes the expired
//! ones and any earlier one of the same plugin.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use super::catalog::Market;
use super::install::{fetch_plugin, install_legacy, inventory_at};
use super::{at, entries, or, string, truthy};
use crate::hub::ctx::HubCtx;
use crate::js;

/// How long a review's staging folder lasts.
pub const TTL_MS: i64 = 30 * 60 * 1000;

/// The line every plain plugin's review carries: nothing signs it.
pub const UNSIGNED: &str = "Unsigned: Buwiz can't check who made it";

/// The record a staging folder keeps (`staged.json`).
const RECORD: &str = "staged.json";

/// A reviewed plugin waiting in its staging folder.
pub struct Staged {
    pub dir: PathBuf,
    pub key: String,
    pub created: i64,
    /// What install records (`root`/`skills`/…, or a legacy `path`).
    pub meta: Value,
    /// `files/<key>` moves to `plugins/<key>` at install (a vendored or
    /// already-cloned legacy plugin has nothing to move).
    pub moves: bool,
}

impl Staged {
    pub fn files(&self) -> PathBuf {
        self.dir.join("files")
    }

    pub fn expired(&self, now: i64) -> bool {
        now - self.created > TTL_MS
    }

    /// Moves the files into `plugins/<key>` and deletes the staging folder.
    pub fn commit(self, m: &Market) -> Result<(), String> {
        if self.moves {
            let (from, to) = (self.files().join(&self.key), m.plugins.join(&self.key));
            let _ = std::fs::remove_dir_all(&to);
            std::fs::rename(&from, &to).map_err(|e| format!("couldn't move the reviewed plugin into place: {e}"))?;
        }
        let _ = std::fs::remove_dir_all(&self.dir);
        Ok(())
    }
}

fn staging_root(m: &Market) -> PathBuf {
    m.plugins.join(".staging")
}

/// A token is 32 lowercase hex characters (it names a folder).
fn valid_token(t: &str) -> bool {
    t.len() == 32 && t.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn new_token() -> String {
    rox_v1::keys::random::<16>().iter().map(|b| format!("{b:02x}")).collect()
}

fn read(dir: &Path) -> Option<Staged> {
    let r: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(RECORD)).ok()?).ok()?;
    Some(Staged {
        dir: dir.to_owned(),
        key: r.get("key")?.as_str()?.to_owned(),
        created: r.get("created")?.as_i64()?,
        meta: r.get("meta").cloned().unwrap_or(Value::Null),
        moves: r.get("moves") == Some(&json!(true)),
    })
}

/// The staging folder of `token`, if it's still there.
pub fn open(m: &Market, token: &str) -> Option<Staged> {
    valid_token(token).then(|| read(&staging_root(m).join(token))).flatten()
}

/// Deletes the expired staging folders (and unreadable ones), and every
/// one of `key`.
pub fn sweep(m: &Market, now: i64, key: Option<&str>) {
    let Ok(list) = std::fs::read_dir(staging_root(m)) else { return };
    for d in list.flatten() {
        let dir = d.path();
        let stale = match read(&dir) {
            Some(s) => s.expired(now) || key == Some(s.key.as_str()),
            None => true,
        };
        if stale {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// `POST /plugins/inspect {name}`: the catalog plugin fetched into a new
/// staging folder, and its review.
pub async fn inspect(m: &Market, ctx: &HubCtx, name: &str) -> Result<Value, String> {
    let e = m.entry_of(name).ok_or_else(|| format!("unknown plugin {name}"))?.clone();
    let key = string(e.get("key"));
    let now = super::now(ctx);
    sweep(m, now, Some(&key));
    let token = new_token();
    let dir = staging_root(m).join(&token);
    let files = dir.join("files");
    std::fs::create_dir_all(&files).map_err(|e| e.to_string())?;
    let fetched: Result<(Value, PathBuf, bool), String> = async {
        if truthy(e.get("legacy")) {
            let p = m.legacy_of(&key).cloned().unwrap_or(Value::Null);
            let root = install_legacy(m, &p, &files).await?;
            Ok((json!({ "path": or(at(p.get("source"), "path"), json!("")) }), root.clone(), root.starts_with(&files)))
        } else {
            let got = fetch_plugin(m, &e, &files).await?;
            let mut meta = json!({ "root": got.root, "skills": got.skills, "repo": or(at(e.get("install"), "repo"), Value::Null), "sha": or(at(e.get("install"), "sha"), Value::Null) });
            if let Some(id) = e.get("id") {
                meta["catalogId"] = id.clone();
            }
            Ok((meta, super::join(&files, &got.root), true))
        }
    }
    .await;
    let (meta, root, moves) = match fetched {
        Ok(f) => f,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(err);
        }
    };
    let record = json!({ "key": key, "created": now, "meta": meta, "moves": moves });
    std::fs::write(dir.join(RECORD), js::stringify(&record)).map_err(|e| e.to_string())?;
    let servers = m.server_configs_at(&key, &root);
    let inv = inventory_at(&files, &root, meta.get("skills"), servers.clone());
    let setup = !m.fields_with(ctx, &key, &servers).is_empty();
    Ok(json!({
        "ok": true,
        "staging": token,
        "name": key,
        "expires": now + TTL_MS,
        "setup": setup,
        "review": review(&e, &inv, &servers),
    }))
}

/// The review: the plugin, its servers (command or URL, the env and header
/// names, never their values), its skills, and what it lists but Buwiz
/// never runs.
fn review(e: &Value, inv: &Value, servers: &Map<String, Value>) -> Value {
    let publisher = or(at(e.get("publisher"), "displayName"), or(at(e.get("publisher"), "name"), json!("")));
    let manifest = inv.get("manifest");
    let names = |k: &str| -> Vec<Value> { entries(inv.get(k)).into_iter().filter_map(|(_, v)| v.get("name").cloned()).collect() };
    let skills: Vec<Value> = entries(inv.get("skills")).into_iter().map(|(_, s)| json!({ "name": s.get("name").cloned().unwrap_or(Value::Null), "description": or(s.get("description"), json!("")) })).collect();
    json!({
        "title": or(e.get("displayName"), e.get("name").cloned().unwrap_or(Value::Null)),
        "publisher": publisher,
        "author": or(at(manifest, "author"), Value::Null),
        "version": or(at(manifest, "version"), Value::Null),
        "description": or(e.get("description"), json!("")),
        "signed": false,
        "unsigned": UNSIGNED,
        "servers": servers.iter().map(|(n, c)| server(n, c)).collect::<Vec<_>>(),
        "skills": skills,
        "listed": { "commands": names("commands"), "agents": names("agents"), "hooks": truthy(inv.get("hooks")), "lsp": truthy(inv.get("lsp")) },
    })
}

/// One server as the review shows it.
fn server(name: &str, cfg: &Value) -> Value {
    let keys = |k: &str| -> Vec<String> { entries(cfg.get(k).filter(|v| v.is_object())).into_iter().map(|(k, _)| k).collect() };
    let text = |k: &str| cfg.get(k).filter(|v| js::truthy(v)).map(js::string_of);
    let url = text("url");
    let command = text("command").map(|c| {
        let args = match cfg.get("args") {
            Some(Value::Array(a)) => a.iter().map(js::string_of).collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        std::iter::once(c).chain(args).collect::<Vec<_>>().join(" ")
    });
    let headers = keys("headers");
    let token = headers.iter().any(|h| h.eq_ignore_ascii_case("authorization"));
    json!({
        "name": name,
        "transport": if url.is_some() { "remote" } else { "command" },
        "url": url,
        "command": if url.is_some() { None } else { command },
        "env": keys("env"),
        "headers": headers,
        "auth": if url.is_some() && !token { "oauth" } else { "none" },
        "unavailable": text("unavailable"),
    })
}
