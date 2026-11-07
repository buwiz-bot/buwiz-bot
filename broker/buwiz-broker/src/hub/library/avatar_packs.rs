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
use std::sync::Mutex;
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
/// All of one read: every file of it, however many packs. Under the 60 seconds the app's HTTP
/// client waits, so a slow read ends in this side's message and not a dropped connection.
const READ_WAIT: Duration = Duration::from_secs(45);
const READ_BYTES: usize = 16 * 1024 * 1024;
const TOO_BIG: &str = "those packs are too big";
const TOO_MANY: &str = "those packs hold too many designs";

/// What one read may still take in bytes. Counted as they arrive, so a file cut off just short
/// of its cap costs as much as one that got there.
pub struct Budget(Mutex<usize>);

impl Default for Budget {
    fn default() -> Self {
        Budget(Mutex::new(READ_BYTES))
    }
}

impl Budget {
    /// Takes `n` bytes, or says the read is over.
    pub fn spend(&self, n: usize) -> Result<(), String> {
        let mut left = self.0.lock().map_err(|_| TOO_BIG.to_owned())?;
        *left = left.checked_sub(n).ok_or(TOO_BIG)?;
        Ok(())
    }
}

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

/// Whether `part` of a GitHub link is a plain name: no escapes, and no dots standing for a folder.
fn plain(part: &str) -> bool {
    !part.chars().all(|c| c == '.') && part.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
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
        // the parser undoes `%2e%2e` before anyone can look at it
        if link.contains('%') {
            return Err("link a GitHub repo, or a branch of one".to_owned());
        }
        let parts: Vec<&str> = u.path().split('/').filter(|p| !p.is_empty()).collect();
        let (owner, repo, branch) = match parts[..] {
            [owner, repo] => (owner, repo, "HEAD"),
            [owner, repo, "tree", branch] => (owner, repo, branch),
            _ => return Err("link a GitHub repo, or a branch of one".to_owned()),
        };
        let repo = repo.strip_suffix(".git").unwrap_or(repo);
        if ![owner, repo, branch].into_iter().all(plain) {
            return Err("link a GitHub repo, or a branch of one".to_owned());
        }
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
pub async fn get_public(url: Url, max: usize, budget: &Budget) -> Result<Vec<u8>, String> {
    tokio::time::timeout(WAIT, follow(url, max, budget)).await.map_err(|_| "the link took too long".to_owned())?
}

async fn follow(mut url: Url, max: usize, budget: &Budget) -> Result<Vec<u8>, String> {
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
            budget.spend(chunk.len())?;
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

/// The pack file `file` names, under the index's own folder. A URL parser undoes `%2e`, so
/// nothing escaped gets through, and the place it resolved to is what's checked last.
fn pack_url(index: &Url, file: &str) -> Option<Url> {
    if !file.ends_with(".json") || file.contains(['\\', '?', '#', ':', '%']) || file.starts_with('/') {
        return None;
    }
    if file.split('/').any(|part| part == ".." || part == "." || part.is_empty()) {
        return None;
    }
    let url = index.join(file).ok()?;
    let folder = &index.path()[..=index.path().rfind('/')?];
    (url.origin() == index.origin() && url.path().starts_with(folder)).then_some(url)
}

/// What an install would refuse of these designs, and why: for a pack's author, before it's
/// published. It runs the same check an install does, so the two can't disagree, and reads
/// nothing.
pub fn refusals(designs: &[Value]) -> Result<Vec<Value>, String> {
    if designs.len() > DESIGNS_MAX {
        return Err(TOO_MANY.to_owned());
    }
    let name = |d: &Value| d.get("name").cloned().unwrap_or(Value::Null);
    Ok(designs.iter().filter_map(|d| clean_design(d).err().map(|why| json!({"name": name(d), "why": why}))).collect())
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

/// `d` with only the fields a design has, at every depth, if it's a sound one, by the rules a
/// bot's own avatar goes by. Everything else in a pack is dropped here: an `image`, a link,
/// whatever a later Bot might act on.
pub fn clean_design(d: &Value) -> Result<Value, String> {
    let name = line(d.get("name"), NAME_MAX);
    if name.is_empty() {
        return Err("no name".to_owned());
    }
    let text = |k: &str| d.get(k).and_then(Value::as_str);
    let set = |k: &str| d.get(k).filter(|v| !v.is_null());
    let errs = avatar_shape::validate_avatar(text("shape"), text("color"), text("outline"), set("layers"), set("eyes"), set("parts"), set("solids"));
    if !errs.is_empty() {
        return Err(errs.join("; "));
    }
    let mut out = Map::new();
    out.insert("name".to_owned(), name.into());
    let why = line(d.get("why"), WHY_MAX);
    if !why.is_empty() {
        out.insert("why".to_owned(), why.into());
    }
    out.insert("shape".to_owned(), d["shape"].clone());
    out.insert("color".to_owned(), d["color"].clone());
    // a kit shape has none of these: what it came with would only ride along
    if text("shape") == Some("custom") {
        let each = |k: &str, of: fn(&Value) -> Value| d.get(k).and_then(Value::as_array).map(|l| Value::Array(l.iter().map(of).collect()));
        let fields = [
            ("outline", set("outline").cloned()),
            ("layers", each("layers", avatar_shape::layer_of)),
            ("eyes", set("eyes").map(avatar_shape::eyes_of)),
            ("parts", each("parts", avatar_shape::part_of)),
            ("solids", each("solids", avatar_shape::solid_of)),
        ];
        out.extend(fields.into_iter().filter_map(|(k, v)| Some((k.to_owned(), v?))));
    }
    Ok(Value::Object(out))
}

/// What came of a read: the sound designs, each refused one with why, and each pack that
/// couldn't be read at all.
#[derive(Debug, Default, PartialEq)]
pub struct Found {
    pub designs: Vec<Value>,
    pub refused: Vec<Value>,
    pub failed: Vec<Value>,
}

/// A pack file's designs into `found`, up to [`DESIGNS_MAX`] in all.
pub fn parse_pack(bytes: &[u8], found: &mut Found) -> Result<(), String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| "a pack isn't JSON".to_owned())?;
    let list = v.get("designs").and_then(Value::as_array).ok_or("a pack has no designs")?;
    for d in list {
        if found.designs.len() + found.refused.len() >= DESIGNS_MAX {
            return Err(TOO_MANY.to_owned());
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

/// The sound designs `pick` means at `link`, all within [`READ_WAIT`]. A pack that can't be read
/// is listed as failed, and the others still count; too much of anything ends the read.
pub async fn read<F, Fut>(link: &str, pick: &str, get: F) -> Result<Value, String>
where
    F: Fn(Url, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, String>>,
{
    read_within(READ_WAIT, link, pick, get).await
}

async fn read_within<F, Fut>(wait: Duration, link: &str, pick: &str, get: F) -> Result<Value, String>
where
    F: Fn(Url, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, String>>,
{
    tokio::time::timeout(wait, read_all(link, pick, get)).await.map_err(|_| "those packs took too long".to_owned())?
}

async fn read_all<F, Fut>(link: &str, pick: &str, get: F) -> Result<Value, String>
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
        match got {
            Err(why) if why == TOO_BIG || why == TOO_MANY => return Err(why),
            Err(why) => found.failed.push(json!({"name": p.title, "why": why})),
            Ok(()) => {}
        }
    }
    if let Some(name) = name {
        found.designs.retain(|d| d["name"].as_str().is_some_and(|n| n.to_lowercase() == name));
        found.refused.retain(|d| d["name"].as_str().is_some_and(|n| n.to_lowercase() == name));
        // it may have been in a pack that couldn't be read
        if found.designs.is_empty() && found.refused.is_empty() && found.failed.is_empty() {
            return Err(format!("nothing called “{}” at that link", pick.trim()));
        }
    }
    Ok(json!({"ok": true, "designs": found.designs, "refused": found.refused, "failed": found.failed}))
}

#[cfg(test)]
mod tests;
