//! plugin_resolve.mjs: find a catalog entry's plugin inside its repository
//! (a marketplace.json entry, a manifest by name, the catalog skills'
//! directory, or the repo's root) and read what it ships.

use std::future::Future;
use std::sync::OnceLock;

use regex_lite::Regex;
use serde_json::{json, Map, Value};

use super::{entries, posix_dirname, posix_join, posix_normalize, truthy};
use crate::js;

fn re(slot: &'static OnceLock<Regex>, p: &str) -> &'static Regex {
    slot.get_or_init(|| Regex::new(p).expect("a valid pattern"))
}

fn manifest_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"(^|/)(?:\.[a-z0-9]+-plugin/|\.plugin/)?plugin\.json$")
}

fn market_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"(^|/)(?:\.[a-z0-9]+-plugin|\.agents/plugins|\.github/plugin|\.plugin)/marketplace\.json$")
}

const MANIFEST_RANK: [&str; 5] = [".cursor-plugin/", ".claude-plugin/", ".grok-plugin/", ".codex-plugin/", ".plugin/"];

fn rank_of(f: &str) -> usize {
    MANIFEST_RANK.iter().position(|m| f.contains(m)).unwrap_or(MANIFEST_RANK.len())
}

fn skip(f: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"(^|/)(node_modules|test|tests|fixtures|examples?)/").is_match(f)
}

/// `clean(p)`: normalized, no "./" lead or trailing slash; `None` above
/// the root.
fn clean(p: &str) -> Option<String> {
    let p = p.strip_prefix("./").unwrap_or(p);
    let n = posix_normalize(p);
    let n = if n == "." { String::new() } else { n };
    let n = n.trim_end_matches('/').to_owned();
    (!n.starts_with("..")).then_some(n)
}

fn plugin_dir_of(manifest: &str) -> String {
    static R: OnceLock<Regex> = OnceLock::new();
    let d = posix_dirname(manifest);
    if re(&R, r"(^|/)\.(?:[a-z0-9]+-)?plugin$").is_match(&d) {
        clean(&posix_dirname(&d)).unwrap_or_default()
    } else if d == "." {
        String::new()
    } else {
        d
    }
}

fn parse(s: Option<String>) -> Value {
    s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)
}

/// What `sourceUrlInfo` reads from a skill's GitHub link.
pub struct SourceInfo {
    pub repo: String,
    pub file: String,
    pub plugin_dir: Option<String>,
}

/// `sourceUrlInfo(url)`.
pub fn source_url_info(url: Option<&Value>) -> Option<SourceInfo> {
    static R: OnceLock<Regex> = OnceLock::new();
    static D: OnceLock<Regex> = OnceLock::new();
    let s = url.filter(|u| js::truthy(u)).map(js::string_of).unwrap_or_default();
    let m = re(&R, r"^https://github\.com/([^/]+/[^/]+)/blob/([^/]+)/(.+)$").captures(&s)?;
    let file = m[3].to_owned();
    let dir = re(&D, r"(?i)^(?:(.*)/)?skills/[^/]+/SKILL\.md$").captures(&file).map(|d| d.get(1).map(|x| x.as_str().to_owned()).unwrap_or_default());
    Some(SourceInfo { repo: format!("https://github.com/{}", &m[1]), file, plugin_dir: dir })
}

/// `repoOf(entry)`.
pub fn repo_of(entry: &Value) -> Option<String> {
    if let Some(u) = entry.get("repositoryUrl").filter(|u| js::truthy(u)) {
        let u = js::string_of(u);
        let u = u.trim_end_matches('/');
        return Some(u.strip_suffix(".git").unwrap_or(u).to_owned());
    }
    entries(entry.get("skills").filter(|s| js::truthy(s))).iter().find_map(|(_, s)| source_url_info(s.get("sourceUrl")).map(|i| i.repo))
}

/// A repo as the resolver reads it: every path at HEAD, and a reader.
pub struct Resolved {
    pub path: String,
    pub mcp_servers: Map<String, Value>,
    pub skills: Vec<(Option<String>, String)>,
}

/// `resolveEntry(entry, {files, read})`.
pub async fn resolve_entry<R, F>(entry: &Value, files: &[String], read: R) -> Result<Resolved, String>
where
    R: Fn(String) -> F,
    F: Future<Output = Option<String>>,
{
    let names: Vec<String> = [entry.get("pluginName"), entry.get("name")].into_iter().filter(|n| truthy(*n)).map(|n| js::string_of(n.unwrap()).to_lowercase()).collect();
    let manifests: Vec<&String> = files.iter().filter(|f| manifest_re().is_match(f) && !skip(f)).collect();
    let manifest_in = |dir: &str| {
        let mut m: Vec<&String> = manifests.iter().copied().filter(|m| plugin_dir_of(m) == dir).collect();
        m.sort_by_key(|f| rank_of(f));
        m.first().map(|s| s.to_string())
    };
    let has_under = |p: &str| files.iter().any(|f| f.starts_with(&format!("{p}/")));
    let (mut path, mut inline): (Option<String>, Option<Value>) = (None, None);

    // 1. a marketplace entry named like the plugin
    let mut markets: Vec<&String> = files.iter().filter(|f| market_re().is_match(f) && !skip(f)).collect();
    markets.sort_by_key(|f| rank_of(f));
    'markets: for mf in markets {
        let m = parse(read(mf.clone()).await);
        let root = super::at(Some(&m), "metadata").and_then(|x| x.get("pluginRoot")).filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
        let root = clean(&root).unwrap_or_default();
        for n in &names {
            let plugins = m.get("plugins").and_then(Value::as_array).cloned().unwrap_or_default();
            let Some(hit) = plugins.iter().find(|p| p.get("name").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default().to_lowercase() == *n) else { continue };
            let source = hit.get("source");
            let src = match source {
                Some(Value::String(s)) => Some(s.clone()),
                Some(s) => match super::nn(s.get("path")) {
                    Some(p) => Some(js::string_of(p)),
                    None if matches!(s.get("source").and_then(Value::as_str), Some("url" | "github")) => None,
                    None => Some(String::new()),
                },
                None => Some(String::new()),
            };
            let Some(src) = src else { continue };
            let Some(p) = clean(&posix_join(&root, &src)) else { continue };
            if !p.is_empty() && !has_under(&p) {
                continue;
            }
            path = Some(p);
            inline = hit.get("mcpServers").filter(|m| m.is_object() || m.is_array()).cloned();
            break 'markets;
        }
    }
    // 2. a manifest whose name matches
    if path.is_none() {
        let mut sorted = manifests.clone();
        sorted.sort_by_key(|f| rank_of(f));
        'names: for n in &names {
            for mf in &sorted {
                let m = parse(read(mf.to_string()).await);
                if m.get("name").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default().to_lowercase() == *n {
                    path = Some(plugin_dir_of(mf));
                    break 'names;
                }
            }
        }
    }
    // 3. the directory the catalog's skill links point into
    if path.is_none() {
        let mut dirs: Vec<String> = Vec::new();
        for (_, s) in entries(entry.get("skills").filter(|s| js::truthy(s))) {
            if let Some(d) = source_url_info(s.get("sourceUrl")).and_then(|i| i.plugin_dir) {
                if !dirs.contains(&d) {
                    dirs.push(d);
                }
            }
        }
        if dirs.len() == 1 {
            path = dirs.pop();
        }
    }
    // 4. a plugin at the repo's root
    static ROOT_SKILL: OnceLock<Regex> = OnceLock::new();
    let root_skill = re(&ROOT_SKILL, r"(?i)^skills/[^/]+/SKILL\.md$");
    if path.is_none() && (manifest_in("").is_some() || files.iter().any(|f| f == "mcp.json" || f == ".mcp.json" || root_skill.is_match(f))) {
        path = Some(String::new());
    }
    let Some(path) = path else { return Err("no plugin manifest in the repository matches this entry".into()) };

    let manifest = match manifest_in(&path) {
        Some(f) => parse(read(f).await),
        None => Value::Null,
    };
    let mut mcp_servers = mcp_of(&path, &manifest, &read, inline).await;
    let mut skills = skills_of(&path, &manifest, files);
    let mut have: Vec<String> = skills.iter().flat_map(|(n, f)| [n.clone().unwrap_or_else(|| "undefined".into()), f.clone()]).collect();
    for (_, s) in entries(entry.get("skills").filter(|s| js::truthy(s))) {
        let Some(i) = source_url_info(s.get("sourceUrl")) else { continue };
        let name = s.get("name").map(js::string_of).unwrap_or_else(|| "undefined".into());
        if !have.contains(&name) && !have.contains(&i.file) && files.contains(&i.file) {
            let parts: Vec<&str> = i.file.split('/').collect();
            skills.push((parts.len().checked_sub(2).map(|k| parts[k].to_owned()), i.file.clone()));
            have.push(i.file);
        }
    }
    // catalog connector names are slugs of the declared keys
    let slug = |n: &str| super::dashes(n, "");
    let wanted: Vec<String> = entries(entry.get("connectors").filter(|c| js::truthy(c))).iter().map(|(_, c)| c.get("name").map(js::string_of).unwrap_or_else(|| "undefined".into())).collect();
    for n in &wanted {
        if mcp_servers.get(n).is_some_and(js::truthy) {
            continue;
        }
        if let Some(k) = mcp_servers.keys().find(|k| !wanted.contains(k) && slug(k) == slug(n)).cloned() {
            let v = mcp_servers.shift_remove(&k).unwrap_or(Value::Null);
            mcp_servers.insert(n.clone(), v);
        }
    }
    let extra: Vec<String> = mcp_servers.keys().filter(|k| !wanted.contains(k)).cloned().collect();
    let lacking: Vec<&String> = wanted.iter().filter(|n| !mcp_servers.get(*n).is_some_and(js::truthy)).collect();
    if extra.len() == 1 && lacking.len() == 1 && wanted.len() == 1 {
        let v = mcp_servers.shift_remove(&extra[0]).unwrap_or(Value::Null);
        mcp_servers.insert(lacking[0].clone(), v);
    }
    Ok(Resolved { path, mcp_servers, skills })
}

/// `mcpOf`: plugin.json's `mcpServers` (inline, or files), else mcp.json
/// or .mcp.json beside it.
async fn mcp_of<R, F>(dir: &str, manifest: &Value, read: &R, inline: Option<Value>) -> Map<String, Value>
where
    R: Fn(String) -> F,
    F: Future<Output = Option<String>>,
{
    let mut servers = Map::new();
    let looks_like = |v: &Value| v.is_object() && (truthy(v.get("url")) || truthy(v.get("command")));
    let take = |j: &Value, servers: &mut Map<String, Value>| {
        let mut s = match j {
            Value::Object(o) => match o.get("mcpServers").filter(|v| js::truthy(v)) {
                Some(v) => Some(v.clone()),
                None => o.get("mcp").filter(|m| m.is_object() || m.is_array()).cloned(),
            },
            _ => None,
        };
        if s.is_none() && (j.is_object() || j.is_array()) && entries(Some(j)).iter().any(|(_, v)| looks_like(v)) {
            s = Some(j.clone());
        }
        if let Some(s @ Value::Object(_)) = s {
            for (k, v) in entries(Some(&s)) {
                if looks_like(&v) {
                    servers.insert(k, v);
                }
            }
        }
    };
    if let Some(inline) = inline {
        take(&json!({ "mcpServers": inline }), &mut servers);
    }
    match manifest.get("mcpServers") {
        Some(decl @ Value::Object(_)) => {
            let _ = decl;
            take(manifest, &mut servers);
        }
        decl => {
            let list: Vec<Value> = match decl {
                Some(Value::Array(a)) => a.clone(),
                Some(v) => vec![v.clone()],
                None => Vec::new(),
            };
            for p in list.iter().filter_map(Value::as_str) {
                if let Some(f) = clean(&posix_join(dir, p)) {
                    take(&parse(read(f).await), &mut servers);
                }
            }
        }
    }
    if servers.is_empty() {
        for f in ["mcp.json", ".mcp.json"] {
            take(&parse(read(if dir.is_empty() { f.to_owned() } else { format!("{dir}/{f}") }).await), &mut servers);
            if !servers.is_empty() {
                break;
            }
        }
    }
    servers
}

/// `skillsOf`: every SKILL.md under the plugin's skill dirs.
fn skills_of(dir: &str, manifest: &Value, files: &[String]) -> Vec<(Option<String>, String)> {
    static SKILL: OnceLock<Regex> = OnceLock::new();
    let skill = re(&SKILL, r"(?i)(^|/)SKILL\.md$");
    let declared: Vec<Value> = match manifest.get("skills") {
        Some(Value::Array(a)) => a.clone(),
        Some(v) => vec![v.clone()],
        None => Vec::new(),
    };
    let mut roots: Vec<Option<String>> = declared.iter().filter_map(Value::as_str).map(|p| clean(&posix_join(dir, p))).collect();
    if roots.is_empty() {
        roots.push(Some(if dir.is_empty() { "skills".into() } else { format!("{dir}/skills") }));
    }
    let mut out = Vec::new();
    for r in roots.into_iter().flatten() {
        let pre = if r.is_empty() { String::new() } else { format!("{r}/") };
        for f in files {
            if !f.starts_with(&pre) || !skill.is_match(f) {
                continue;
            }
            let rest: Vec<&str> = f[pre.len()..].split('/').collect();
            if rest.len() == 2 {
                out.push((Some(rest[0].to_owned()), f.clone()));
            } else if rest.len() == 1 && !r.is_empty() {
                out.push((r.rsplit('/').next().map(str::to_owned), f.clone()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn mkt_resolve_through_a_marketplace_entry_a_manifest_or_the_root() {
        let fs = files(&[
            ".cursor-plugin/marketplace.json",
            "plugins/docs-canvas/.cursor-plugin/plugin.json",
            "plugins/docs-canvas/skills/docs-canvas/SKILL.md",
            "plugins/docs-canvas/.mcp.json",
            "plugins/other/plugin.json",
        ]);
        let read = |p: String| async move {
            Some(match p.as_str() {
                ".cursor-plugin/marketplace.json" => r#"{"plugins":[{"name":"docs-canvas","source":"plugins/docs-canvas"}]}"#.to_owned(),
                "plugins/docs-canvas/.mcp.json" => r#"{"mcpServers":{"canvas":{"command":"node"}}}"#.to_owned(),
                "plugins/other/plugin.json" => r#"{"name":"other"}"#.to_owned(),
                _ => "{}".to_owned(),
            })
        };
        let r = resolve_entry(&json!({"name": "docs-canvas", "connectors": []}), &fs, read).await.unwrap();
        assert_eq!(r.path, "plugins/docs-canvas");
        assert_eq!(r.skills, vec![(Some("docs-canvas".to_owned()), "plugins/docs-canvas/skills/docs-canvas/SKILL.md".to_owned())]);
        assert_eq!(Value::Object(r.mcp_servers), json!({"canvas": {"command": "node"}}));
        let r = resolve_entry(&json!({"name": "other"}), &fs, read).await.unwrap();
        assert_eq!(r.path, "plugins/other");
        // one server, one connector, different names: re-keyed
        let r = resolve_entry(&json!({"name": "docs-canvas", "connectors": [{"name": "Canvas Tool"}]}), &fs, read).await.unwrap();
        assert_eq!(Value::Object(r.mcp_servers), json!({"Canvas Tool": {"command": "node"}}));
        assert_eq!(resolve_entry(&json!({"name": "nope"}), &files(&["README.md"]), read).await.err().unwrap(), "no plugin manifest in the repository matches this entry");
        assert_eq!(resolve_entry(&json!({"name": "nope"}), &files(&["skills/a/SKILL.md"]), read).await.unwrap().path, "");
    }

    #[test]
    fn mkt_source_urls_and_repos() {
        let i = source_url_info(Some(&json!("https://github.com/o/r/blob/abc/x/y/skills/s/SKILL.md"))).unwrap();
        assert_eq!((i.repo.as_str(), i.file.as_str(), i.plugin_dir.as_deref()), ("https://github.com/o/r", "x/y/skills/s/SKILL.md", Some("x/y")));
        assert_eq!(source_url_info(Some(&json!("https://github.com/o/r/blob/abc/skills/s/SKILL.md"))).unwrap().plugin_dir.as_deref(), Some(""));
        assert_eq!(repo_of(&json!({"repositoryUrl": "https://github.com/o/r.git/"})).as_deref(), Some("https://github.com/o/r"));
        assert_eq!(clean("./a/b/"), Some("a/b".into()));
        assert_eq!(clean("../x"), None);
        assert_eq!(plugin_dir_of("x/.claude-plugin/plugin.json"), "x");
        assert_eq!(plugin_dir_of(".claude-plugin/plugin.json"), "");
    }
}
