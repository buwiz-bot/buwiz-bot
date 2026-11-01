//! A mini app from a GitHub release (drafts' install contract, part 1):
//! the repository a URL names, its release (`releases/latest` or
//! `releases/tags/<tag>`), the signed index `buwiz-release.json` verified
//! against the trust store, then the manifest and this platform's binary,
//! each checked against the index's sha256, written to
//! `<data>/v1/apps/<id>/<version>/` through a temp dir and a rename. What
//! comes out is the spawn spec `Host::inspect` takes; nothing is installed.
//!
//! The API base is a parameter (production: [`GITHUB_API`]); plain http is
//! allowed only when the base itself is http (tests on 127.0.0.1).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use url::Url;

use super::http::{self, Req};
use crate::v1::trust::Trust;

pub const GITHUB_API: &str = "https://api.github.com";
pub const INDEX_ASSET: &str = "buwiz-release.json";
pub const MANIFEST_FILE: &str = "manifest.signed.json";
const SMALL_MAX: usize = 1024 * 1024;
const BINARY_MAX: usize = 200 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const USER_AGENT: &str = "buwiz-bot";

/// The repository (and tag) an install URL names.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub owner: String,
    pub repo: String,
    pub tag: Option<String>,
}

fn name_ok(s: &str, extra: &[char]) -> bool {
    !s.is_empty() && s.len() <= 100 && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || extra.contains(&c))
}

/// `https://github.com/<owner>/<repo>` (optionally `/releases/tag/<tag>`,
/// a trailing `/` or `.git`) or `<owner>/<repo>`.
pub fn parse_url(url: &str) -> Result<Source, String> {
    let bad = || format!("not a GitHub repository: {} (send https://github.com/<owner>/<repo> or <owner>/<repo>)", url.chars().take(120).collect::<String>());
    let s = url.trim();
    let path = match s.strip_prefix("https://github.com/") {
        Some(p) => p,
        None if !s.contains(':') && !s.starts_with('/') => s,
        None => return Err(bad()),
    };
    let path = path.strip_suffix('/').unwrap_or(path);
    let parts: Vec<&str> = path.split('/').collect();
    let (owner, repo, tag) = match parts.as_slice() {
        [o, r] => (*o, *r, None),
        [o, r, "releases", "tag", t] if s.starts_with("https://") => (*o, *r, Some(*t)),
        _ => return Err(bad()),
    };
    let repo = if tag.is_none() { repo.strip_suffix(".git").unwrap_or(repo) } else { repo };
    if !name_ok(owner, &[]) || !name_ok(repo, &['.', '_']) || tag.is_some_and(|t| !name_ok(t, &['.', '_', '+'])) {
        return Err(bad());
    }
    Ok(Source { owner: owner.to_owned(), repo: repo.to_owned(), tag: tag.map(str::to_owned) })
}

/// This build's platform as release indexes name it (`macos-aarch64`).
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// One downloadable file of the index.
#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub asset: String,
    pub sha256: String,
}

/// A verified `buwiz-release.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct Index {
    pub id: String,
    pub version: String,
    pub key_id: String,
    pub manifest: Asset,
    /// this platform's
    pub binary: Asset,
    pub args: Vec<String>,
    /// `run.env`, unexpanded, in order
    pub env: Vec<(String, String)>,
    /// `[{key, label, secret}]`
    pub needs_env: Vec<Value>,
}

fn sha_ok(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn asset_of(v: Option<&Value>, what: &str) -> Result<Asset, String> {
    let asset = v.and_then(|a| a.get("asset")).and_then(Value::as_str).filter(|a| name_ok(a, &['.', '_'])).ok_or_else(|| format!("release index: {what}.asset must be a file name"))?;
    let sha256 = v.and_then(|a| a.get("sha256")).and_then(Value::as_str).filter(|s| sha_ok(s)).ok_or_else(|| format!("release index: {what}.sha256 must be 64 lowercase hex"))?;
    Ok(Asset { asset: asset.to_owned(), sha256: sha256.to_owned() })
}

/// Verifies index text: `v` 1, the signature by `publisher.key_id` from the
/// trust store, the id approved for that key, a build for `platform`.
pub fn verify_index(text: &str, trust: &Trust, platform: &str) -> Result<Index, String> {
    let v = rox_v1::jcs::parse_ijson(text).map_err(|e| format!("release index: not JSON: {e}"))?;
    let Value::Object(o) = &v else { return Err("release index: must be a JSON object".into()) };
    if o.get("v") != Some(&json!(1)) {
        return Err("release index: v must be 1".into());
    }
    let id = o.get("id").and_then(Value::as_str).filter(|i| rox_v1::ids::plugin_id_ok(i)).ok_or("release index: id must be a plugin id")?;
    let version = o.get("version").filter(|x| crate::v1::manifest::is_semver(Some(*x))).and_then(Value::as_str).ok_or("release index: version must be semver")?;
    let key_id = o.get("publisher").and_then(|p| p.get("key_id")).and_then(Value::as_str).filter(|k| !k.is_empty()).ok_or("release index: publisher.key_id is missing")?;
    let sig = o.get("signature").ok_or("release index: it is not signed")?;
    if sig.get("alg").and_then(Value::as_str) != Some("ed25519") {
        return Err("release index: signature.alg must be ed25519".into());
    }
    if sig.get("key_id").and_then(Value::as_str) != Some(key_id) {
        return Err("release index: signature.key_id must equal publisher.key_id".into());
    }
    let Some(key) = trust.publishers.get(key_id) else { return Err(format!("release index: {key_id} is not a trusted publisher key")) };
    let mut unsigned = o.clone();
    unsigned.shift_remove("signature");
    let value = sig.get("value").and_then(Value::as_str).unwrap_or_default();
    if rox_v1::keys::verify(&key.public_key, &Value::Object(unsigned), value).is_err() {
        return Err("release index: the signature does not verify with the publisher key".into());
    }
    if !trust.is_approved(id, key_id) {
        return Err(format!("{id} is not an approved plugin for key {key_id}"));
    }
    let manifest = asset_of(o.get("manifest"), "manifest")?;
    let Some(bin) = o.get("binaries").and_then(|b| b.get(platform)) else { return Err(format!("no build for {platform}")) };
    let binary = asset_of(Some(bin), &format!("binaries.{platform}"))?;
    let run = o.get("run").cloned().unwrap_or(json!({}));
    let args = match run.get("args") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) if a.iter().all(Value::is_string) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        _ => return Err("release index: run.args must be an array of strings".into()),
    };
    let env = match run.get("env") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(m)) if m.values().all(Value::is_string) => m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned())).collect(),
        _ => return Err("release index: run.env must map names to strings".into()),
    };
    let mut needs_env = Vec::new();
    for n in o.get("needs_env").and_then(Value::as_array).into_iter().flatten() {
        let key = n.get("key").and_then(Value::as_str).filter(|k| !k.is_empty()).ok_or("release index: needs_env[].key must be a name")?;
        let label = n.get("label").and_then(Value::as_str).unwrap_or(key);
        needs_env.push(json!({"key": key, "label": label, "secret": n.get("secret") == Some(&Value::Bool(true))}));
    }
    Ok(Index { id: id.to_owned(), version: version.to_owned(), key_id: key_id.to_owned(), manifest, binary, args, env, needs_env })
}

/// `{manifest}` and `{dir}` in `run.env` values; nothing else.
pub fn expand(env: &[(String, String)], manifest: &Path, dir: &Path) -> Vec<(String, String)> {
    let (m, d) = (manifest.display().to_string(), dir.display().to_string());
    env.iter().map(|(k, v)| (k.clone(), v.replace("{manifest}", &m).replace("{dir}", &d))).collect()
}

/// The downloaded manifest against the index: id, version, key id.
fn check_manifest(text: &[u8], ix: &Index) -> Result<(), String> {
    let m: Value = serde_json::from_slice(text).map_err(|_| "the release's manifest is not JSON".to_owned())?;
    let key = m.get("publisher").and_then(|p| p.get("key_id")).and_then(Value::as_str);
    for (what, got, want) in [("id", m.get("id").and_then(Value::as_str), &ix.id), ("version", m.get("version").and_then(Value::as_str), &ix.version), ("publisher.key_id", key, &ix.key_id)] {
        if got != Some(want.as_str()) {
            return Err(format!("the release's manifest {what} is {}, the index says {want}", got.unwrap_or("missing")));
        }
    }
    Ok(())
}

/// A GET that follows up to 5 redirects to any host, https only (http
/// too when `plain` — a test base).
async fn get(url: &str, accept: &str, plain: bool) -> Result<http::Resp, String> {
    let mut url = Url::parse(url).map_err(|_| format!("bad URL {url}"))?;
    for _ in 0..=MAX_REDIRECTS {
        if url.scheme() != "https" && !(plain && url.scheme() == "http") {
            return Err(format!("refused a non-https URL ({})", url.scheme()));
        }
        let res = http::send(&Req::get(url.clone()).header("user-agent", USER_AGENT).header("accept", accept)).await?;
        match http::redirect_target(&res) {
            Some(next) => {
                res.cancel();
                url = next;
            }
            None if res.ok() => return Ok(res),
            None => return Err(format!("{} answered {}", url.host_str().unwrap_or("the server"), res.status)),
        }
    }
    Err("too many redirects".into())
}

fn too_big(res: &http::Resp, cap: usize) -> bool {
    res.header("content-length").and_then(|l| l.parse::<u64>().ok()).is_some_and(|n| n > cap as u64)
}

/// A body of at most `cap` bytes.
async fn small(url: &str, accept: &str, plain: bool, cap: usize) -> Result<Vec<u8>, String> {
    let mut res = get(url, accept, plain).await?;
    let over = || format!("{url} is over {} KiB", cap / 1024);
    if too_big(&res, cap) {
        return Err(over());
    }
    let mut out = Vec::new();
    while let Some(c) = res.chunk().await? {
        out.extend_from_slice(&c);
        if out.len() > cap {
            return Err(over());
        }
    }
    Ok(out)
}

/// A body streamed to `file` (at most `cap` bytes); its sha256.
async fn to_file(url: &str, plain: bool, cap: usize, file: &Path) -> Result<String, String> {
    let mut res = get(url, "application/octet-stream", plain).await?;
    if too_big(&res, cap) {
        return Err(format!("the binary is over {} MiB", cap / 1024 / 1024));
    }
    let mut f = std::fs::File::create(file).map_err(|e| e.to_string())?;
    let (mut h, mut n) = (Sha256::new(), 0usize);
    while let Some(c) = res.chunk().await? {
        n += c.len();
        if n > cap {
            return Err(format!("the binary is over {} MiB", cap / 1024 / 1024));
        }
        h.update(&c);
        f.write_all(&c).map_err(|e| e.to_string())?;
    }
    f.flush().map_err(|e| e.to_string())?;
    Ok(hex(&h.finalize()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn file_sha(p: &Path) -> Option<String> {
    let mut f = std::fs::File::open(p).ok()?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).ok()?;
    Some(hex(&h.finalize()))
}

/// A release ready to inspect.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub index: Index,
    /// the release page (`…/releases/tag/<tag>`)
    pub source: String,
    pub dir: PathBuf,
    pub command: String,
    pub args: Vec<String>,
    /// `run.env` expanded
    pub env: Vec<(String, String)>,
}

impl Fetched {
    /// `{command, args, env}` for `Host::inspect`.
    pub fn spec(&self) -> Value {
        let env: Map<String, Value> = self.env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        json!({"command": self.command, "args": self.args, "env": env})
    }
}

fn asset_url(release: &Value, name: &str) -> Result<String, String> {
    release
        .get("assets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|a| a.get("name").and_then(Value::as_str) == Some(name))
        .and_then(|a| a.get("browser_download_url").and_then(Value::as_str))
        .map(str::to_owned)
        .ok_or_else(|| format!("the release has no asset {name}"))
}

/// Whether `dir` already holds this index's files.
fn complete(dir: &Path, ix: &Index) -> bool {
    file_sha(&dir.join(MANIFEST_FILE)).as_deref() == Some(ix.manifest.sha256.as_str()) && file_sha(&dir.join(&ix.binary.asset)).as_deref() == Some(ix.binary.sha256.as_str())
}

/// Resolves, downloads and verifies `src`'s release under `data`, the
/// API at `api` (production: [`GITHUB_API`]).
pub async fn fetch(api: &str, src: &Source, trust: &Trust, data: &Path) -> Result<Fetched, String> {
    let plain = api.starts_with("http://");
    let api = api.trim_end_matches('/');
    let at = match &src.tag {
        Some(t) => format!("{api}/repos/{}/{}/releases/tags/{t}", src.owner, src.repo),
        None => format!("{api}/repos/{}/{}/releases/latest", src.owner, src.repo),
    };
    let body = small(&at, "application/vnd.github+json", plain, SMALL_MAX).await.map_err(|e| {
        if e.ends_with("answered 404") {
            format!("{}/{} has no published release{}", src.owner, src.repo, src.tag.as_ref().map(|t| format!(" {t}")).unwrap_or_default())
        } else {
            e
        }
    })?;
    let release: Value = serde_json::from_slice(&body).map_err(|_| "GitHub's release answer is not JSON".to_owned())?;
    let tag = release.get("tag_name").and_then(Value::as_str).map(str::to_owned).or_else(|| src.tag.clone()).unwrap_or_default();
    let source = format!("https://github.com/{}/{}/releases/tag/{tag}", src.owner, src.repo);
    let text = small(&asset_url(&release, INDEX_ASSET)?, "application/octet-stream", plain, SMALL_MAX).await?;
    let ix = verify_index(&String::from_utf8_lossy(&text), trust, &platform())?;
    let base = data.join("v1").join("apps").join(&ix.id);
    let dir = base.join(&ix.version);
    if !complete(&dir, &ix) {
        std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
        let tmp = base.join(format!(".tmp-{}-{}", ix.version, rox_v1::keys::random_id()));
        let staged = stage(&release, &ix, plain, &tmp).await;
        if let Err(e) = staged {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }
        let _ = std::fs::remove_dir_all(&dir);
        if let Err(e) = std::fs::rename(&tmp, &dir) {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(format!("could not move the release into place: {e}"));
        }
    }
    let manifest = std::fs::read(dir.join(MANIFEST_FILE)).map_err(|e| e.to_string())?;
    check_manifest(&manifest, &ix)?;
    let env = expand(&ix.env, &dir.join(MANIFEST_FILE), &dir);
    Ok(Fetched { command: dir.join(&ix.binary.asset).display().to_string(), args: ix.args.clone(), env, source, dir, index: ix })
}

/// The manifest and binary into `tmp`, checked.
async fn stage(release: &Value, ix: &Index, plain: bool, tmp: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::create_dir_all(tmp).map_err(|e| e.to_string())?;
    let manifest = small(&asset_url(release, &ix.manifest.asset)?, "application/octet-stream", plain, SMALL_MAX).await?;
    if hex(&Sha256::digest(&manifest)) != ix.manifest.sha256 {
        return Err(format!("{} does not match the index's sha256", ix.manifest.asset));
    }
    check_manifest(&manifest, ix)?;
    std::fs::write(tmp.join(MANIFEST_FILE), &manifest).map_err(|e| e.to_string())?;
    let bin = tmp.join(&ix.binary.asset);
    let sha = to_file(&asset_url(release, &ix.binary.asset)?, plain, BINARY_MAX, &bin).await?;
    if sha != ix.binary.sha256 {
        return Err(format!("{} does not match the index's sha256", ix.binary.asset));
    }
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const SEED: [u8; 32] = [9; 32];
    pub const KEY: &str = "test-rel";

    pub fn trust(approved: &[&str]) -> Trust {
        let public = rox_v1::keys::SessionKey::from_seed(&rox_v1::keys::b64(&SEED)).unwrap().public();
        Trust::from_value(&json!({
            "publishers": {KEY: {"alg": "ed25519", "publisher": "app.buwiz", "public_key": public}},
            "approved_plugins": approved.iter().map(|id| json!({"id": id, "publisher_key_id": KEY})).collect::<Vec<_>>(),
        }))
    }

    /// A signed index for `id` 1.0.0 with a build for `platform`.
    pub fn index(id: &str, platform: &str, manifest_sha: &str, bin_sha: &str, edit: impl FnOnce(&mut Value)) -> String {
        let mut ix = json!({
            "v": 1, "id": id, "version": "1.0.0", "publisher": {"key_id": KEY},
            "manifest": {"asset": MANIFEST_FILE, "sha256": manifest_sha},
            "binaries": {platform: {"asset": "fx-bin", "sha256": bin_sha}},
            "run": {"args": ["mcp"], "env": {"FX_MANIFEST": "{manifest}", "HOME_DIR": "{dir}/x", "LIT": "{other}"}},
            "needs_env": [{"key": "FX_TOKEN", "label": "Token", "secret": true}],
        });
        edit(&mut ix);
        let signed = crate::v1::signing::sign_manifest(&ix, &rox_v1::keys::b64(&SEED), &[]).unwrap();
        signed.to_string()
    }

    #[test]
    fn urls_name_a_repository_and_maybe_a_tag() {
        let ok = |u: &str, tag: Option<&str>| assert_eq!(parse_url(u), Ok(Source { owner: "buwiz-app".into(), repo: "google-workspace".into(), tag: tag.map(str::to_owned) }), "{u}");
        ok("https://github.com/buwiz-app/google-workspace", None);
        ok("https://github.com/buwiz-app/google-workspace/", None);
        ok("https://github.com/buwiz-app/google-workspace.git", None);
        ok(" buwiz-app/google-workspace ", None);
        ok("https://github.com/buwiz-app/google-workspace/releases/tag/v1.1.0", Some("v1.1.0"));
        ok("https://github.com/buwiz-app/google-workspace/releases/tag/v1.1.0/", Some("v1.1.0"));
        for bad in [
            "http://github.com/a/b",
            "https://gitlab.com/a/b",
            "https://github.com/a",
            "https://github.com/a/b/c",
            "https://github.com/a/b/releases",
            "https://github.com/a/b/releases/latest",
            "https://github.com/../b",
            "https://github.com/a/b/releases/tag/x%2F..",
            "a/b/releases/tag/v1",
            "/a/b",
            "a",
            "",
            "https://github.com/a/b?x=1",
            "git@github.com:a/b.git",
        ] {
            assert!(parse_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_index_verifies_only_signed_approved_and_built_for_this_platform() {
        let (m, b) = ("a".repeat(64), "b".repeat(64));
        let t = trust(&["app.buwiz.fixture"]);
        let ix = verify_index(&index("app.buwiz.fixture", "macos-aarch64", &m, &b, |_| {}), &t, "macos-aarch64").unwrap();
        assert_eq!((ix.id.as_str(), ix.version.as_str(), ix.key_id.as_str()), ("app.buwiz.fixture", "1.0.0", KEY));
        assert_eq!(ix.binary, Asset { asset: "fx-bin".into(), sha256: b.clone() });
        assert_eq!(ix.args, ["mcp"]);
        assert_eq!(ix.needs_env, [json!({"key": "FX_TOKEN", "label": "Token", "secret": true})]);
        let err = |text: String, trust: &Trust| verify_index(&text, trust, "macos-aarch64").unwrap_err();
        // a changed byte after signing
        let tampered = index("app.buwiz.fixture", "macos-aarch64", &m, &b, |_| {}).replace("\"mcp\"", "\"cli\"");
        assert!(err(tampered, &t).contains("does not verify"));
        assert!(err(index("app.buwiz.other", "macos-aarch64", &m, &b, |_| {}), &t).contains("is not an approved plugin"));
        let other_key = index("app.buwiz.fixture", "macos-aarch64", &m, &b, |ix| ix["publisher"]["key_id"] = json!("gc-2026-01"));
        assert!(err(other_key, &t).contains("gc-2026-01 is not a trusted publisher key"));
        let mut swapped: Value = serde_json::from_str(&index("app.buwiz.fixture", "macos-aarch64", &m, &b, |_| {})).unwrap();
        swapped["signature"]["key_id"] = json!("other");
        assert!(err(swapped.to_string(), &t).contains("signature.key_id must equal"));
        assert_eq!(err(index("app.buwiz.fixture", "linux-x86_64", &m, &b, |_| {}), &t), "no build for macos-aarch64");
        assert!(err(index("app.buwiz.fixture", "macos-aarch64", &m, &b, |ix| ix["v"] = json!(2)), &t).contains("v must be 1"));
        assert!(err(index("app.buwiz.fixture", "macos-aarch64", "XYZ", &b, |_| {}), &t).contains("manifest.sha256"));
        let escape = index("app.buwiz.fixture", "macos-aarch64", &m, &b, |ix| ix["binaries"]["macos-aarch64"]["asset"] = json!("../bin"));
        assert!(err(escape, &t).contains("must be a file name"));
        let mut unsigned: Value = serde_json::from_str(&index("app.buwiz.fixture", "macos-aarch64", &m, &b, |_| {})).unwrap();
        unsigned.as_object_mut().unwrap().shift_remove("signature");
        assert!(err(unsigned.to_string(), &t).contains("not signed"));
    }

    #[test]
    fn run_env_expands_manifest_and_dir_only() {
        let env = vec![("A".to_owned(), "{manifest}".to_owned()), ("B".to_owned(), "{dir}/x:{dir}".to_owned()), ("C".to_owned(), "{other} {MANIFEST}".to_owned())];
        let out = expand(&env, Path::new("/d/m.json"), Path::new("/d"));
        assert_eq!(out, [("A".to_owned(), "/d/m.json".to_owned()), ("B".to_owned(), "/d/x:/d".to_owned()), ("C".to_owned(), "{other} {MANIFEST}".to_owned())]);
    }

    #[test]
    fn the_manifest_must_be_the_index_s_id_version_and_key() {
        let ix = verify_index(&index("app.buwiz.fixture", "p", &"a".repeat(64), &"b".repeat(64), |_| {}), &trust(&["app.buwiz.fixture"]), "p").unwrap();
        let m = |id: &str, v: &str, k: &str| json!({"id": id, "version": v, "publisher": {"key_id": k}}).to_string();
        assert!(check_manifest(m("app.buwiz.fixture", "1.0.0", KEY).as_bytes(), &ix).is_ok());
        assert!(check_manifest(m("app.buwiz.other", "1.0.0", KEY).as_bytes(), &ix).unwrap_err().contains("id is app.buwiz.other"));
        assert!(check_manifest(m("app.buwiz.fixture", "1.0.1", KEY).as_bytes(), &ix).unwrap_err().contains("version is 1.0.1"));
        assert!(check_manifest(m("app.buwiz.fixture", "1.0.0", "k2").as_bytes(), &ix).unwrap_err().contains("publisher.key_id is k2"));
    }
}
