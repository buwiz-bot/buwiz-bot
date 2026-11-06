//! Avatar packs: designs someone published, read from a link. A link names a GitHub repo or an
//! `https` file; `index.json` lists its packs, and each pack is a JSON file of designs:
//!
//! ```json
//! {"version": 1, "packs": [{"id": "animals", "title": "Animals", "file": "packs/animals.json",
//!                           "groups": ["originals"]}]}
//! {"designs": [{"name": "Fox", "shape": "custom", "color": "#E8772E", "outline": "M…"}]}
//! ```
//!
//! All of it is untrusted, and the link may come from a model. So this reads only public
//! `https` hosts and refuses the rest by where the connection really went, caps every size,
//! keeps only the fields a design has, puts each through [`avatar_shape::validate_avatar`], and
//! runs nothing. It only reads: the Bot alone writes "Your designs", so the list has one writer.

use std::future::Future;
use std::net::IpAddr;
use std::time::Duration;

use serde_json::{json, Map, Value};
use url::Url;

use super::avatar_shape;
use crate::helper::http;

/// Where packs come from when a person names no link.
pub const DEFAULT_LINK: &str = "https://github.com/buwiz-bot/avatars";

const INDEX_MAX: usize = 256 * 1024;
const PACK_MAX: usize = 4 * 1024 * 1024;
const PACKS_MAX: usize = 100;
const DESIGNS_MAX: usize = 1000;
const NAME_MAX: usize = 80;
const WHY_MAX: usize = 300;
const HOPS: usize = 3;
const WAIT: Duration = Duration::from_secs(20);

/// A pack an index lists.
#[derive(Clone, Debug, PartialEq)]
pub struct Pack {
    pub id: String,
    pub title: String,
    pub file: Url,
    pub groups: Vec<String>,
}

/// Why a URL can't be read: `https` on its default port, by name, with no credentials.
fn refused_url(u: &Url) -> Option<&'static str> {
    if u.scheme() != "https" {
        return Some("a pack link must be https");
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Some("a pack link can't carry a password");
    }
    if u.port().is_some() {
        return Some("a pack link can't name a port");
    }
    match u.host() {
        Some(url::Host::Domain(d)) if d.contains('.') && !d.ends_with(".local") => None,
        _ => Some("a pack link must name a public host"),
    }
}

/// The index a link means: `index.json` at the top of a GitHub repo, on its default branch or
/// the one in a `/tree/<branch>` link, or the `.json` file a link names.
pub fn index_url(link: &str) -> Result<Url, String> {
    let link = link.trim();
    let link = if link.is_empty() { DEFAULT_LINK } else { link };
    let u = Url::parse(link).map_err(|_| "that isn't a link".to_owned())?;
    if let Some(why) = refused_url(&u) {
        return Err(why.to_owned());
    }
    let index = if u.host_str() == Some("github.com") {
        let parts: Vec<&str> = u.path().split('/').filter(|p| !p.is_empty()).collect();
        let (owner, repo, branch) = match parts[..] {
            [owner, repo] => (owner, repo, "HEAD"),
            [owner, repo, "tree", branch] => (owner, repo, branch),
            _ => return Err("link a GitHub repo, or a branch of one".to_owned()),
        };
        let repo = repo.strip_suffix(".git").unwrap_or(repo);
        format!("https://raw.githubusercontent.com/{owner}/{repo}/{branch}/index.json")
    } else if u.path().ends_with(".json") {
        u.to_string()
    } else {
        return Err("link a GitHub repo or an index .json file".to_owned());
    };
    Url::parse(&index).map_err(|_| "that isn't a link".to_owned())
}

/// Whether `ip` is on the public internet: none of this Mac, its network or the ranges kept
/// for something else.
pub fn public_ip(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v) => {
            let [a, b, c, _] = v.octets();
            !(v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_broadcast()
                || v.is_documentation()
                || v.is_unspecified()
                || v.is_multicast()
                || a == 0
                || a >= 240
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 198 && (b == 18 || b == 19)))
        }
        IpAddr::V6(v) => {
            let first = v.segments()[0];
            !(v.is_loopback()
                || v.is_unspecified()
                || v.is_multicast()
                || first & 0xfe00 == 0xfc00
                || first & 0xffc0 == 0xfe80
                || (first == 0x2001 && v.segments()[1] == 0x0db8))
        }
    }
}

/// `url`'s body, up to `max` bytes, from public hosts only: each hop is looked at before it's
/// asked, and what the connection really reached after.
pub async fn get_public(url: Url, max: usize) -> Result<Vec<u8>, String> {
    tokio::time::timeout(WAIT, follow(url, max)).await.map_err(|_| "the link took too long".to_owned())?
}

async fn follow(mut url: Url, max: usize) -> Result<Vec<u8>, String> {
    let lost = || "couldn't read that link".to_owned();
    for _ in 0..=HOPS {
        if let Some(why) = refused_url(&url) {
            return Err(why.to_owned());
        }
        let host = url.host_str().ok_or_else(lost)?.to_owned();
        let found: Vec<_> = tokio::net::lookup_host((host, 443)).await.map_err(|_| lost())?.collect();
        if found.is_empty() || !found.iter().all(|a| public_ip(a.ip())) {
            return Err("a pack link must name a public host".to_owned());
        }
        let req = http::Req::get(url.clone()).header("user-agent", "buwiz-bot");
        let mut res = http::send(&req).await.map_err(|_| lost())?;
        // a name can change between the look above and the connection
        if !res.peer.is_some_and(|p| public_ip(p.ip())) {
            res.cancel();
            return Err("a pack link must name a public host".to_owned());
        }
        if let Some(next) = http::redirect_target(&res) {
            res.cancel();
            url = next;
            continue;
        }
        if !res.ok() {
            let status = res.status;
            res.cancel();
            return Err(if status == 404 { "no packs at that link".to_owned() } else { lost() });
        }
        let mut body = Vec::new();
        while let Some(chunk) = res.chunk().await.map_err(|_| lost())? {
            if body.len() + chunk.len() > max {
                res.cancel();
                return Err("that file is too big".to_owned());
            }
            body.extend_from_slice(&chunk);
        }
        return Ok(body);
    }
    Err("that link redirects too much".to_owned())
}

/// A short line of text from a pack: control characters gone, cut to `max` characters.
fn line(v: Option<&Value>, max: usize) -> String {
    let s = v.and_then(Value::as_str).unwrap_or_default();
    s.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_owned()
}

/// The pack file `file` names, under the index's own folder.
fn pack_url(index: &Url, file: &str) -> Option<Url> {
    if !file.ends_with(".json") || file.contains(['\\', '?', '#', ':']) || file.starts_with('/') {
        return None;
    }
    if file.split('/').any(|part| part == ".." || part == "." || part.is_empty()) {
        return None;
    }
    index.join(file).ok()
}

/// The packs in `index.json`. One with a bad entry isn't read; the others are.
pub fn parse_index(bytes: &[u8], index: &Url) -> Result<Vec<Pack>, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| "that index isn't JSON".to_owned())?;
    if v.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("that index is a version this Bot can't read".to_owned());
    }
    let list = v.get("packs").and_then(Value::as_array).ok_or("that index lists no packs")?;
    if list.len() > PACKS_MAX {
        return Err("that index lists too many packs".to_owned());
    }
    let mut packs = Vec::new();
    for p in list {
        let id = line(p.get("id"), NAME_MAX);
        let file = p.get("file").and_then(Value::as_str).and_then(|f| pack_url(index, f));
        let (false, Some(file)) = (id.is_empty(), file) else { continue };
        let title = line(p.get("title"), NAME_MAX);
        let groups = p.get("groups").and_then(Value::as_array).map(|g| g.iter().take(16).map(|x| line(Some(x), NAME_MAX)).filter(|x| !x.is_empty()).collect()).unwrap_or_default();
        packs.push(Pack { title: if title.is_empty() { id.clone() } else { title }, id, file, groups });
    }
    Ok(packs)
}

/// `d` with only the fields a design has, if it's a sound one. Everything else in a pack is
/// dropped here: an `image`, a link, whatever a later Bot might act on.
pub fn clean_design(d: &Value) -> Result<Value, String> {
    let name = line(d.get("name"), NAME_MAX);
    if name.is_empty() {
        return Err("no name".to_owned());
    }
    let text = |k: &str| d.get(k).and_then(Value::as_str);
    let errs = avatar_shape::validate_avatar(text("shape"), text("color"), text("outline"), d.get("layers"), d.get("eyes"), d.get("parts"), d.get("solids"));
    if !errs.is_empty() {
        return Err(errs.join("; "));
    }
    let mut out = Map::new();
    out.insert("name".to_owned(), name.into());
    let why = line(d.get("why"), WHY_MAX);
    if !why.is_empty() {
        out.insert("why".to_owned(), why.into());
    }
    for k in ["shape", "color", "outline", "layers", "eyes", "parts", "solids"] {
        if let Some(v) = d.get(k).filter(|v| !v.is_null()) {
            out.insert(k.to_owned(), v.clone());
        }
    }
    Ok(Value::Object(out))
}

/// What came of a read: the sound designs, and each refused one with why.
#[derive(Debug, Default, PartialEq)]
pub struct Found {
    pub designs: Vec<Value>,
    pub refused: Vec<Value>,
}

/// A pack file's designs into `found`, up to [`DESIGNS_MAX`] in all.
pub fn parse_pack(bytes: &[u8], found: &mut Found) -> Result<(), String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| "a pack isn't JSON".to_owned())?;
    let list = v.get("designs").and_then(Value::as_array).ok_or("a pack has no designs")?;
    for d in list {
        if found.designs.len() + found.refused.len() >= DESIGNS_MAX {
            return Err("those packs hold too many designs".to_owned());
        }
        match clean_design(d) {
            Ok(design) => found.designs.push(design),
            Err(why) => found.refused.push(json!({"name": line(d.get("name"), NAME_MAX), "why": why})),
        }
    }
    Ok(())
}

/// Which packs `pick` means: all of them for nothing or `all`, the ones it names by id, title
/// or group, or all of them to look through for a design of that name.
fn chosen<'a>(packs: &'a [Pack], pick: &str) -> (Vec<&'a Pack>, Option<String>) {
    let want = pick.trim().to_lowercase();
    if want.is_empty() || want == "all" {
        return (packs.iter().collect(), None);
    }
    let named: Vec<&Pack> = packs
        .iter()
        .filter(|p| p.id.to_lowercase() == want || p.title.to_lowercase() == want || p.groups.iter().any(|g| g.to_lowercase() == want))
        .collect();
    if named.is_empty() { (packs.iter().collect(), Some(want)) } else { (named, None) }
}

/// The packs at `link`, as a person would pick from them.
pub async fn list<F, Fut>(link: &str, get: F) -> Result<Value, String>
where
    F: Fn(Url, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, String>>,
{
    let index = index_url(link)?;
    let packs = parse_index(&get(index.clone(), INDEX_MAX).await?, &index)?;
    let packs: Vec<Value> = packs.iter().map(|p| json!({"id": p.id, "title": p.title, "groups": p.groups})).collect();
    Ok(json!({"ok": true, "packs": packs}))
}

/// The sound designs `pick` means at `link`. A pack that can't be read is reported with the
/// refused, and the others still count.
pub async fn read<F, Fut>(link: &str, pick: &str, get: F) -> Result<Value, String>
where
    F: Fn(Url, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, String>>,
{
    let index = index_url(link)?;
    let packs = parse_index(&get(index.clone(), INDEX_MAX).await?, &index)?;
    let (packs, name) = chosen(&packs, pick);
    let mut found = Found::default();
    for p in packs {
        let got = match get(p.file.clone(), PACK_MAX).await {
            Ok(bytes) => parse_pack(&bytes, &mut found),
            Err(e) => Err(e),
        };
        if let Err(why) = got {
            found.refused.push(json!({"name": p.title, "why": why}));
        }
    }
    if let Some(name) = name {
        found.designs.retain(|d| d["name"].as_str().is_some_and(|n| n.to_lowercase() == name));
        found.refused.retain(|d| d["name"].as_str().is_some_and(|n| n.to_lowercase() == name));
        if found.designs.is_empty() && found.refused.is_empty() {
            return Err(format!("nothing called “{}” at that link", pick.trim()));
        }
    }
    Ok(json!({"ok": true, "designs": found.designs, "refused": found.refused}))
}

#[cfg(test)]
mod tests;
