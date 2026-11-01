//! A mini app package (extension format 2.1 §1): a plugin folder whose
//! `plugin.json` carries one signed `buwiz` block. The block names the
//! mini app's `mcpServers` entry, its accounts, and every other file of the
//! folder by sha256 (binaries per platform). The signature is ed25519 by
//! `buwiz.publisher.key_id` over JCS of the whole plugin.json with
//! `buwiz.signature` removed, so commands, skills and every hash are
//! covered. Trust is the 2.0 manifest's: a trusted key, the id approved
//! for it. The served `buwiz://manifest` keeps its own signature.

use std::path::Path;

use serde_json::{json, Map, Value};

use super::trust::Trust;
use super::{ed_verify, get, signed_bytes, V1Error};
use crate::js;

pub const PLUGIN_FILE: &str = "plugin.json";
pub const BLOCK: &str = "buwiz";
pub const PLUGIN_JSON_MAX: usize = 1024 * 1024;
pub const ACCOUNTS_MAX: usize = 8;
pub const ACCOUNT_LABEL_MAX: usize = 60;
pub const SCOPES_MAX: usize = 16;
pub const FILES_MAX: usize = 256;
pub const FILES_BYTES_MAX: u64 = 50 * 1024 * 1024;
pub const BINARY_BYTES_MAX: u64 = 200 * 1024 * 1024;
const PATH_MAX: usize = 256;

type Res<T> = Result<T, V1Error>;

/// One downloadable file of the package: where it goes, its release asset.
#[derive(Debug, Clone, PartialEq)]
pub struct File {
    pub path: String,
    pub asset: String,
    pub sha256: String,
}

/// A declared account (§4).
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub id: String,
    /// `google` or `oauth:<issuer URL>`
    pub provider: String,
    pub label: String,
    pub scopes: Vec<String>,
    pub required: bool,
}

impl Account {
    pub fn json(&self) -> Value {
        json!({"id": self.id, "provider": self.provider, "label": self.label, "scopes": self.scopes, "required": self.required})
    }
}

/// A well-formed `buwiz` block (and its plugin.json).
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub plugin: Value,
    pub protocol: String,
    pub id: String,
    pub short: String,
    pub version: String,
    pub server: String,
    pub key_id: String,
    pub accounts: Vec<Account>,
    pub files: Vec<File>,
    /// this platform's, when it has one
    pub binary: Option<File>,
}

fn bad(why: impl std::fmt::Display) -> V1Error {
    V1Error::new("manifest_invalid", format!("plugin.json: {why}"))
}

fn sha_ok(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A release asset name (GitHub's are flat).
fn asset_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || "-._".contains(c))
}

/// A relative `/`-separated install path: no `..`, no leading `/`, each
/// segment `[A-Za-z0-9._-]{1,100}`.
pub fn path_ok(p: &str) -> bool {
    p.len() <= PATH_MAX && !p.is_empty() && p.split('/').all(|s| !s.is_empty() && s.len() <= 100 && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)))
}

/// `^[a-z][a-z0-9-]{0,31}$`.
pub fn account_id_ok(s: &str) -> bool {
    let mut cs = s.chars();
    s.len() <= 32 && cs.next().is_some_and(|c| c.is_ascii_lowercase()) && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `google`, or `oauth:` and an https issuer (http on a loopback host:
/// tests).
pub fn provider_ok(p: &str) -> bool {
    if p == "google" {
        return true;
    }
    let Some(issuer) = p.strip_prefix("oauth:") else { return false };
    match url::Url::parse(issuer) {
        Ok(u) => u.scheme() == "https" || (u.scheme() == "http" && matches!(u.host_str(), Some("127.0.0.1" | "localhost"))),
        Err(_) => false,
    }
}

fn text<'a>(o: &'a Map<String, Value>, k: &str, at: &str) -> Res<&'a str> {
    o.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| bad(format!("{at}.{k} must be a non-empty string")))
}

fn file_of(v: &Value, at: &str, path: Option<&str>) -> Res<File> {
    let Some(o) = v.as_object() else { return Err(bad(format!("{at} must be an object"))) };
    let path = match path {
        Some(p) => p.to_owned(),
        None => text(o, "path", at)?.to_owned(),
    };
    if !path_ok(&path) {
        return Err(bad(format!("{at}: {} is not a relative path of [A-Za-z0-9._-] segments", js::slice_utf16(&path, 80))));
    }
    let asset = o.get("asset").and_then(Value::as_str).filter(|a| asset_ok(a)).ok_or_else(|| bad(format!("{at}.asset must be a file name")))?;
    let sha256 = o.get("sha256").and_then(Value::as_str).filter(|s| sha_ok(s)).ok_or_else(|| bad(format!("{at}.sha256 must be 64 lowercase hex")))?;
    Ok(File { path, asset: asset.to_owned(), sha256: sha256.to_owned() })
}

/// `name` says who in a refusal: the block's id, when it has one.
fn protocol_ok(p: &str, host: &str, name: &str) -> Res<()> {
    let two = |s: &str| {
        let mut it = s.split('.').map(|x| x.parse::<u64>().ok());
        (it.next().flatten(), it.next().flatten())
    };
    let (Some(maj), Some(min)) = two(p) else { return Err(bad("buwiz.protocol must be a semver string")) };
    if !super::manifest::is_semver(Some(&json!(p))) {
        return Err(bad("buwiz.protocol must be a semver string"));
    }
    if super::manifest::pre_release(p) {
        return Err(V1Error::new("protocol_unsupported", super::manifest::pre_release_message(name, p)));
    }
    let (hmaj, hmin) = two(host);
    if Some(maj) != hmaj || Some(min) > hmin {
        return Err(V1Error::new("protocol_unsupported", format!("the package is for protocol {p}; this Buwiz Bot supports {host}")));
    }
    Ok(())
}

/// The block's shape (§1's table), signature not checked; `platform`
/// picks `binary`.
pub fn parse(raw: &str, host: &str, platform: &str) -> Res<Package> {
    if raw.len() > PLUGIN_JSON_MAX {
        return Err(bad(format!("is over {} KiB", PLUGIN_JSON_MAX / 1024)));
    }
    let plugin = buwiz_protocol::jcs::parse_ijson(raw).map_err(|e| bad(format!("is not I-JSON: {e}")))?;
    let Some(top) = plugin.as_object() else { return Err(bad("must be a JSON object")) };
    let Some(b) = top.get(BLOCK).and_then(Value::as_object) else { return Err(bad("has no buwiz block")) };
    let protocol = text_of(b, "protocol")?;
    let name = b.get("id").and_then(Value::as_str).filter(|id| buwiz_protocol::ids::plugin_id_ok(id)).unwrap_or("this mini app");
    protocol_ok(&protocol, host, name)?;
    let id = text_of(b, "id")?;
    if !buwiz_protocol::ids::plugin_id_ok(&id) {
        return Err(bad("buwiz.id must be a plugin id"));
    }
    let short = text_of(b, "short")?;
    if !buwiz_protocol::ids::short_ok(&short) {
        return Err(bad("buwiz.short must be a short name"));
    }
    let version = text_of(b, "version")?;
    if !super::manifest::is_semver(Some(&json!(version))) {
        return Err(bad("buwiz.version must be semver"));
    }
    if top.get("version").and_then(Value::as_str) != Some(version.as_str()) {
        return Err(bad(format!("version must equal buwiz.version ({version})")));
    }
    let server = text_of(b, "server")?;
    let entry = top.get("mcpServers").and_then(|m| get(m, &server)).and_then(Value::as_object).ok_or_else(|| bad(format!("buwiz.server {server} is not an mcpServers entry")))?;
    let stdio = !entry.contains_key("url") && entry.get("type").or(entry.get("transport")).is_none_or(|t| t == "stdio");
    if !stdio || !entry.get("command").and_then(Value::as_str).is_some_and(|c| !c.is_empty()) {
        return Err(bad(format!("mcpServers.{server} must be a stdio server with a command")));
    }
    if entry.get("args").is_some_and(|a| !a.as_array().is_some_and(|a| a.iter().all(Value::is_string))) {
        return Err(bad(format!("mcpServers.{server}.args must be an array of strings")));
    }
    if entry.get("env").is_some_and(|e| !e.as_object().is_some_and(|m| m.values().all(Value::is_string))) {
        return Err(bad(format!("mcpServers.{server}.env must map names to strings")));
    }
    let accounts = accounts_of(b.get("accounts"))?;
    let publisher = b.get("publisher").and_then(Value::as_object).ok_or_else(|| bad("buwiz.publisher must be an object"))?;
    let key_id = text(publisher, "key_id", "buwiz.publisher")?.to_owned();
    if !super::grammar_ok("keyId", &key_id) {
        return Err(bad("buwiz.publisher.key_id is not a key id"));
    }
    let mut files = Vec::new();
    let listed = match b.get("files") {
        None => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err(bad("buwiz.files must be an object")),
    };
    if listed.len() > FILES_MAX {
        return Err(bad(format!("buwiz.files lists over {FILES_MAX} files")));
    }
    for (path, f) in &listed {
        if path == PLUGIN_FILE {
            return Err(bad("buwiz.files must not list plugin.json"));
        }
        files.push(file_of(f, &format!("buwiz.files.{}", js::slice_utf16(path, 80)), Some(path))?);
    }
    let Some(bins) = b.get("binaries").and_then(Value::as_object) else { return Err(bad("buwiz.binaries must be an object")) };
    let mut binary = None;
    for (plat, f) in bins {
        let file = file_of(f, &format!("buwiz.binaries.{}", js::slice_utf16(plat, 40)), None)?;
        if file.path == PLUGIN_FILE || files.iter().any(|x| x.path == file.path) {
            return Err(bad(format!("buwiz.binaries.{plat}.path is also a listed file")));
        }
        if plat == platform {
            binary = Some(file);
        }
    }
    Ok(Package { plugin: plugin.clone(), protocol, id, short, version, server, key_id, accounts, files, binary })
}

fn text_of(b: &Map<String, Value>, k: &str) -> Res<String> {
    text(b, k, "buwiz").map(str::to_owned)
}

fn accounts_of(v: Option<&Value>) -> Res<Vec<Account>> {
    let list = match v {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(a)) => a,
        Some(_) => return Err(bad("buwiz.accounts must be an array")),
    };
    if list.len() > ACCOUNTS_MAX {
        return Err(bad(format!("buwiz.accounts holds over {ACCOUNTS_MAX} accounts")));
    }
    let mut out: Vec<Account> = Vec::new();
    for (i, a) in list.iter().enumerate() {
        let at = format!("buwiz.accounts[{i}]");
        let Some(o) = a.as_object() else { return Err(bad(format!("{at} must be an object"))) };
        let id = text(o, "id", &at)?;
        if !account_id_ok(id) || out.iter().any(|x| x.id == id) {
            return Err(bad(format!("{at}.id must be a unique ^[a-z][a-z0-9-]{{0,31}}$ name")));
        }
        let provider = text(o, "provider", &at)?;
        if provider == "buwiz" {
            return Err(V1Error::new("not_supported", format!("plugin.json: {at}.provider buwiz is reserved for the Buwiz account and not supported yet")));
        }
        if !provider_ok(provider) {
            return Err(bad(format!("{at}.provider must be google or oauth:<https issuer URL>")));
        }
        let label = text(o, "label", &at)?;
        if label.encode_utf16().count() > ACCOUNT_LABEL_MAX {
            return Err(bad(format!("{at}.label is over {ACCOUNT_LABEL_MAX} characters")));
        }
        let scopes = match o.get("scopes") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(s)) if s.len() <= SCOPES_MAX && s.iter().all(|x| x.as_str().is_some_and(|x| !x.is_empty() && x.len() <= 512 && !x.contains(char::is_whitespace))) => {
                s.iter().filter_map(Value::as_str).map(str::to_owned).collect()
            }
            Some(_) => return Err(bad(format!("{at}.scopes must be at most {SCOPES_MAX} scope strings"))),
        };
        let required = match o.get("required") {
            None => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(bad(format!("{at}.required must be a boolean"))),
        };
        out.push(Account { id: id.to_owned(), provider: provider.to_owned(), label: label.to_owned(), scopes, required });
    }
    Ok(out)
}

/// The block's signature and trust: the key trusted (its publisher the
/// block's), `(id, key_id)` approved, ed25519 over JCS of plugin.json
/// without `buwiz.signature`.
pub fn check_signature(p: &Package, trust: &Trust) -> Res<()> {
    let b = &p.plugin[BLOCK];
    let Some(key) = trust.publishers.get(&p.key_id) else {
        return Err(bad(format!("buwiz.publisher.key_id: {} is not a trusted publisher key", p.key_id)));
    };
    if !key.publisher.is_empty() && b.pointer("/publisher/id").and_then(Value::as_str) != Some(key.publisher.as_str()) {
        return Err(bad(format!("buwiz.publisher.id: key {} belongs to {}", p.key_id, key.publisher)));
    }
    if !trust.is_approved(&p.id, &p.key_id) {
        return Err(bad(format!("buwiz.id: {} is not on the approved list for key {}", p.id, p.key_id)));
    }
    let Some(s) = b.get("signature").and_then(Value::as_object) else { return Err(bad("buwiz.signature is missing: the package is not signed")) };
    if s.get("alg").and_then(Value::as_str) != Some("ed25519") {
        return Err(bad("buwiz.signature.alg must be \"ed25519\""));
    }
    if s.get("key_id").and_then(Value::as_str) != Some(p.key_id.as_str()) {
        return Err(bad("buwiz.signature.key_id must equal buwiz.publisher.key_id"));
    }
    let value = s.get("value").and_then(Value::as_str).unwrap_or_default();
    let bytes = signed_bytes(&unsigned(&p.plugin)).map_err(bad)?;
    if !ed_verify(&key.public_key, &bytes, value) {
        return Err(bad("buwiz.signature does not verify with the publisher key"));
    }
    Ok(())
}

/// plugin.json without `buwiz.signature`: what the signature covers.
pub fn unsigned(plugin: &Value) -> Value {
    let mut out = plugin.clone();
    if let Some(b) = out.get_mut(BLOCK).and_then(Value::as_object_mut) {
        b.shift_remove("signature");
    }
    out
}

/// [`parse`] and [`check_signature`]: a package Buwiz Bot will install.
pub fn verify(text: &str, trust: &Trust, host: &str, platform: &str) -> Res<Package> {
    let p = parse(text, host, platform)?;
    check_signature(&p, trust)?;
    Ok(p)
}

/// The served `buwiz://manifest` against the block: id, short, version.
pub fn check_served(p: &Package, m: &Value) -> Res<()> {
    for (k, want) in [("id", &p.id), ("short", &p.short), ("version", &p.version)] {
        let got = get(m, k).and_then(Value::as_str);
        if got != Some(want.as_str()) {
            return Err(V1Error::new("manifest_invalid", format!("the served manifest's {k} is {}; plugin.json's buwiz.{k} is {want}", got.unwrap_or("missing"))));
        }
    }
    Ok(())
}

/// `${PLUGIN_ROOT}` (and the Claude and Cursor names for it) as `root`.
pub fn expand_root(s: &str, root: &Path) -> String {
    let r = root.display().to_string();
    s.replace("${PLUGIN_ROOT}", &r).replace("${CLAUDE_PLUGIN_ROOT}", &r).replace("${CURSOR_PLUGIN_ROOT}", &r)
}

/// `{command, args, env}` of `mcpServers[server]`, the root expanded.
pub fn spawn_body(p: &Package, root: &Path) -> Value {
    let e = &p.plugin["mcpServers"][&p.server];
    let x = |v: &Value| json!(expand_root(v.as_str().unwrap_or_default(), root));
    let args: Vec<Value> = e.get("args").and_then(Value::as_array).into_iter().flatten().map(x).collect();
    let env: Map<String, Value> = e.get("env").and_then(Value::as_object).into_iter().flatten().map(|(k, v)| (k.clone(), x(v))).collect();
    json!({"command": x(&e["command"]), "args": args, "env": env})
}

/// Each listed file and this platform's binary at `root` with its sha256.
pub fn check_files(p: &Package, root: &Path) -> Res<()> {
    let bin = p.binary.as_ref().ok_or_else(|| V1Error::new("not_supported", format!("{} has no build for {}", p.id, crate::helper::release::platform())))?;
    for f in p.files.iter().chain(std::iter::once(bin)) {
        if crate::helper::release::file_sha(&root.join(&f.path)).as_deref() != Some(f.sha256.as_str()) {
            return Err(V1Error::new("manifest_invalid", format!("{} does not match plugin.json's sha256", f.path)));
        }
    }
    Ok(())
}

/// The skill roots plugin.json names (`skills`: a path or a list; default
/// `skills/`).
fn skill_roots(plugin: &Value) -> Vec<String> {
    let roots: Vec<String> = match plugin.get("skills") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        _ => vec!["skills/".to_owned()],
    };
    roots.into_iter().map(|r| r.trim_start_matches("./").trim_end_matches('/').to_owned()).filter(|r| path_ok(r)).collect()
}

/// `[{name, description, file}]`: each listed `<root>/<name>/SKILL.md`,
/// its description from the front matter, `file` under `root`.
pub fn skills_of(p: &Package, root: &Path) -> Vec<Value> {
    let roots = skill_roots(&p.plugin);
    let mut out = Vec::new();
    for f in &p.files {
        let Some((dir, name)) = f.path.strip_suffix("/SKILL.md").and_then(|d| d.rsplit_once('/')) else { continue };
        if !roots.iter().any(|r| r == dir) {
            continue;
        }
        let file = root.join(&f.path);
        let head = std::fs::read_to_string(&file).map(|t| js::slice_utf16(&t, 3000)).unwrap_or_default();
        out.push(json!({"name": name, "description": crate::hub::market::install::front_desc(&head), "file": file.display().to_string()}));
    }
    out
}

/// plugin.json in `dir`, verified (signature, trust, this platform).
pub fn load(dir: &Path, trust: &Trust, host: &str) -> Res<Package> {
    let text = std::fs::read_to_string(dir.join(PLUGIN_FILE)).map_err(|e| V1Error::new("not_found", format!("{}: {e}", dir.join(PLUGIN_FILE).display())))?;
    verify(&text, trust, host, &crate::helper::release::platform())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::helper::release::tests::{trust, KEY, SEED};

    pub const ID: &str = "app.buwiz.fixture";

    /// A plugin.json for `ID` 1.0.0 with one account, one file and a
    /// binary for `platform`, edited, then signed.
    pub fn plugin_json(platform: &str, edit: impl FnOnce(&mut Value)) -> Value {
        let mut p = json!({
            "name": "fixture", "version": "1.0.0", "description": "The fixture",
            "mcpServers": {"fx": {"command": "${PLUGIN_ROOT}/bin/fx", "args": ["mcp"], "env": {"FX_MANIFEST": "${PLUGIN_ROOT}/manifest.signed.json"}}},
            "skills": ["skills/"],
            "buwiz": {
                "protocol": "1.0.0", "id": ID, "short": "fx", "version": "1.0.0", "server": "fx",
                "accounts": [{"id": "google", "provider": "google", "label": "Google account", "scopes": ["https://www.googleapis.com/auth/gmail.readonly"], "required": true}],
                "files": {"skills/triage/SKILL.md": {"asset": "skills.triage.SKILL.md", "sha256": "a".repeat(64)}},
                "binaries": {platform: {"path": "bin/fx", "asset": "fx-bin", "sha256": "b".repeat(64)}},
                "publisher": {"id": "app.buwiz", "key_id": KEY},
            }
        });
        edit(&mut p);
        crate::v1::signing::sign_block(&p, BLOCK, &buwiz_protocol::keys::b64(&SEED)).unwrap()
    }

    const HOST: &str = super::super::manifest::HOST_PROTOCOL;

    type Edit = Box<dyn FnOnce(&mut Value)>;

    fn refused(edit: impl FnOnce(&mut Value)) -> V1Error {
        verify(&plugin_json("p", edit).to_string(), &trust(&[ID]), HOST, "p").unwrap_err()
    }

    #[test]
    fn a_signed_package_parses_with_its_platform_binary_and_accounts() {
        let p = verify(&plugin_json("macos-aarch64", |_| {}).to_string(), &trust(&[ID]), HOST, "macos-aarch64").unwrap();
        assert_eq!((p.id.as_str(), p.short.as_str(), p.version.as_str(), p.server.as_str()), (ID, "fx", "1.0.0", "fx"));
        assert_eq!(p.binary, Some(File { path: "bin/fx".into(), asset: "fx-bin".into(), sha256: "b".repeat(64) }));
        assert_eq!(p.files, [File { path: "skills/triage/SKILL.md".into(), asset: "skills.triage.SKILL.md".into(), sha256: "a".repeat(64) }]);
        assert_eq!(p.accounts[0].json(), json!({"id": "google", "provider": "google", "label": "Google account", "scopes": ["https://www.googleapis.com/auth/gmail.readonly"], "required": true}));
        // another platform: parsed, no binary (the install refuses it)
        assert_eq!(verify(&plugin_json("linux-x86_64", |_| {}).to_string(), &trust(&[ID]), HOST, "macos-aarch64").unwrap().binary, None);
        let body = spawn_body(&p, Path::new("/apps/fx/1.0.0"));
        assert_eq!(body, json!({"command": "/apps/fx/1.0.0/bin/fx", "args": ["mcp"], "env": {"FX_MANIFEST": "/apps/fx/1.0.0/manifest.signed.json"}}));
    }

    #[test]
    fn the_signature_covers_every_field_of_plugin_json() {
        let t = trust(&[ID]);
        let good = plugin_json("p", |_| {});
        assert!(verify(&good.to_string(), &t, HOST, "p").is_ok());
        let tampered: [(&str, Edit); 5] = [
            ("command", Box::new(|p| p["mcpServers"]["fx"]["command"] = json!("/bin/sh"))),
            ("args", Box::new(|p| p["mcpServers"]["fx"]["args"] = json!(["evil"]))),
            ("file sha", Box::new(|p| p["buwiz"]["files"]["skills/triage/SKILL.md"]["sha256"] = json!("c".repeat(64)))),
            ("binary sha", Box::new(|p| p["buwiz"]["binaries"]["p"]["sha256"] = json!("c".repeat(64)))),
            ("description", Box::new(|p| p["description"] = json!("changed"))),
        ];
        for (what, edit) in tampered {
            let mut p = good.clone();
            edit(&mut p);
            let e = verify(&p.to_string(), &t, HOST, "p").unwrap_err();
            assert!(e.message.contains("does not verify"), "{what}: {e:?}");
        }
        let mut unsigned = good.clone();
        unsigned["buwiz"].as_object_mut().unwrap().shift_remove("signature");
        assert!(verify(&unsigned.to_string(), &t, HOST, "p").unwrap_err().message.contains("not signed"));
        let mut swapped = good.clone();
        swapped["buwiz"]["signature"]["key_id"] = json!("other");
        assert!(verify(&swapped.to_string(), &t, HOST, "p").unwrap_err().message.contains("must equal buwiz.publisher.key_id"));
    }

    #[test]
    fn trust_and_approval_are_the_2_0_rules() {
        let text = plugin_json("p", |_| {}).to_string();
        assert!(verify(&text, &trust(&["app.buwiz.other"]), HOST, "p").unwrap_err().message.contains("is not on the approved list"));
        assert!(verify(&text, &Trust::default(), HOST, "p").unwrap_err().message.contains("is not a trusted publisher key"));
        let other = plugin_json("p", |p| p["buwiz"]["publisher"]["id"] = json!("dev.other"));
        assert!(verify(&other.to_string(), &trust(&[ID]), HOST, "p").unwrap_err().message.contains("belongs to app.buwiz"));
    }

    #[test]
    fn a_package_names_protocol_1_0_and_2_x_was_its_pre_release() {
        assert_eq!(HOST, "1.0.0");
        for p in ["1.0.0", "1.0.2"] {
            assert_eq!(verify(&plugin_json("p", |b| b["buwiz"]["protocol"] = json!(p)).to_string(), &trust(&[ID]), HOST, "p").unwrap().protocol, p);
        }
        for p in ["2.0.0", "2.1.0"] {
            let e = refused(|b| b["buwiz"]["protocol"] = json!(p));
            assert_eq!((e.code.as_str(), e.message), ("protocol_unsupported", format!("{ID} uses the pre-release mini app protocol {p}; install its 1.0 version")));
        }
        for p in ["1.1.0", "3.0.0"] {
            let e = refused(|b| b["buwiz"]["protocol"] = json!(p));
            assert_eq!((e.code.as_str(), e.message), ("protocol_unsupported", format!("the package is for protocol {p}; this Buwiz Bot supports 1.0.0")));
        }
    }

    #[test]
    fn the_block_is_held_to_its_rules() {
        let cases: Vec<(Edit, &str)> = vec![
            (Box::new(|p| p["buwiz"]["protocol"] = json!("2.9.0")), "supports"),
            (Box::new(|p| p["buwiz"]["protocol"] = json!("1.1.0")), "supports"),
            (Box::new(|p| p["buwiz"]["protocol"] = json!("3.1.0")), "supports"),
            (Box::new(|p| p["buwiz"]["id"] = json!("Fixture")), "buwiz.id must be a plugin id"),
            (Box::new(|p| p["buwiz"]["version"] = json!("1.0.1")), "version must equal buwiz.version"),
            (Box::new(|p| p["buwiz"]["server"] = json!("none")), "is not an mcpServers entry"),
            (Box::new(|p| p["mcpServers"]["fx"] = json!({"url": "https://x.test/mcp"})), "must be a stdio server"),
            (Box::new(|p| p["mcpServers"]["fx"]["args"] = json!([1])), "args must be an array of strings"),
            (Box::new(|p| p["buwiz"]["files"] = json!({"../x": {"asset": "x", "sha256": "a".repeat(64)}})), "is not a relative path"),
            (Box::new(|p| p["buwiz"]["files"] = json!({"/etc/x": {"asset": "x", "sha256": "a".repeat(64)}})), "is not a relative path"),
            (Box::new(|p| p["buwiz"]["files"] = json!({"a//b": {"asset": "x", "sha256": "a".repeat(64)}})), "is not a relative path"),
            (Box::new(|p| p["buwiz"]["files"] = json!({"a b": {"asset": "x", "sha256": "a".repeat(64)}})), "is not a relative path"),
            (Box::new(|p| p["buwiz"]["files"] = json!({"plugin.json": {"asset": "x", "sha256": "a".repeat(64)}})), "must not list plugin.json"),
            (Box::new(|p| p["buwiz"]["files"] = json!({"x": {"asset": "a/b", "sha256": "a".repeat(64)}})), "asset must be a file name"),
            (Box::new(|p| p["buwiz"]["files"] = json!({"x": {"asset": "x", "sha256": "A".repeat(64)}})), "sha256 must be 64 lowercase hex"),
            (Box::new(|p| p["buwiz"]["files"] = json!({"bin/fx": {"asset": "x", "sha256": "a".repeat(64)}})), "is also a listed file"),
            (Box::new(|p| p["buwiz"]["binaries"]["p"]["path"] = json!("../fx")), "is not a relative path"),
            (Box::new(|p| p["buwiz"]["accounts"][0]["id"] = json!("Google")), "unique ^[a-z]"),
            (Box::new(|p| p["buwiz"]["accounts"][0]["provider"] = json!("oauth:http://evil.test")), "google or oauth:"),
            (Box::new(|p| p["buwiz"]["accounts"][0]["provider"] = json!("github")), "google or oauth:"),
            (Box::new(|p| p["buwiz"]["accounts"][0]["provider"] = json!("buwiz")), "reserved for the Buwiz account"),
            (Box::new(|p| p["buwiz"]["accounts"][0]["label"] = json!("l".repeat(61))), "label is over 60"),
            (Box::new(|p| p["buwiz"]["accounts"][0]["scopes"] = json!(vec!["s"; 17])), "at most 16 scope strings"),
            (Box::new(|p| p["buwiz"]["accounts"][0]["required"] = json!("yes")), "required must be a boolean"),
            (Box::new(|p| p["buwiz"]["accounts"] = json!(vec![json!({"id": "a", "provider": "google", "label": "A"}); 2])), "unique ^[a-z]"),
        ];
        for (edit, want) in cases {
            let e = refused(edit);
            assert!(e.message.contains(want), "{want}: {e:?}");
        }
        assert!(verify(r#"{"name":"x","version":"1.0.0"}"#, &trust(&[ID]), HOST, "p").unwrap_err().message.contains("has no buwiz block"));
        let nine: Vec<Value> = (0..9).map(|i| json!({"id": format!("a{i}"), "provider": "google", "label": "A"})).collect();
        assert!(refused(|p| p["buwiz"]["accounts"] = json!(nine)).message.contains("over 8 accounts"));
        let many: Map<String, Value> = (0..257).map(|i| (format!("f{i}"), json!({"asset": format!("f{i}"), "sha256": "a".repeat(64)}))).collect();
        assert!(refused(|p| p["buwiz"]["files"] = Value::Object(many)).message.contains("over 256 files"));
        assert!(refused(|p| p["description"] = json!("x".repeat(PLUGIN_JSON_MAX))).message.contains("is over 1024 KiB"));
        assert!(provider_ok("oauth:https://auth.example.com/tenant") && provider_ok("oauth:http://127.0.0.1:9/") && !provider_ok("oauth:not a url"));
    }

    #[test]
    fn the_served_manifest_must_be_the_blocks_id_short_and_version() {
        let p = parse(&plugin_json("p", |_| {}).to_string(), HOST, "p").unwrap();
        assert!(check_served(&p, &json!({"id": ID, "short": "fx", "version": "1.0.0"})).is_ok());
        for (k, v) in [("id", "app.buwiz.other"), ("short", "fy"), ("version", "1.0.1")] {
            let mut m = json!({"id": ID, "short": "fx", "version": "1.0.0"});
            m[k] = json!(v);
            let e = check_served(&p, &m).unwrap_err();
            assert_eq!(e.message, format!("the served manifest's {k} is {v}; plugin.json's buwiz.{k} is {}", if k == "id" { ID } else if k == "short" { "fx" } else { "1.0.0" }));
        }
    }

    #[test]
    fn skills_are_the_listed_skill_md_files_under_a_root() {
        let dir = std::env::temp_dir().join(format!("rox-package-skills-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("skills/triage")).unwrap();
        std::fs::write(dir.join("skills/triage/SKILL.md"), "---\nname: triage\ndescription: Triage the inbox\n---\nbody").unwrap();
        let p = parse(&plugin_json("p", |p| p["buwiz"]["files"]["docs/x/SKILL.md"] = json!({"asset": "d", "sha256": "a".repeat(64)})).to_string(), HOST, "p").unwrap();
        let file = dir.join("skills/triage/SKILL.md").display().to_string();
        assert_eq!(skills_of(&p, &dir), [json!({"name": "triage", "description": "Triage the inbox", "file": file})]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
