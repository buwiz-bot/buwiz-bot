//! Installs from git (`repoView`, `fetchPlugin`, the legacy clone), what an
//! installed plugin ships (`pluginInventory`, `skillFile`,
//! `pluginSkillsInject`), and its connectors as `mcp_servers` rows
//! (`upsertConnector` over the part of `mcpUpsertServer` it uses).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use regex_lite::Regex;
use rusqlite::OptionalExtension;
use serde_json::{json, Map, Value};

use super::catalog::{Market, Materialized};
use super::{at, entries, nn, parse, posix_dirname, truthy};
use crate::hub::ctx::HubCtx;
use crate::js;

/// config.mjs's `git(args)`: 120 s; stdout, or execFile's error message.
pub async fn git(args: &[String]) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let failed = |stderr: &[u8]| format!("Command failed: git {}\n{}", args.join(" "), String::from_utf8_lossy(stderr));
    let child = cmd.spawn().map_err(|_| "spawn git ENOENT".to_owned())?;
    match tokio::time::timeout(Duration::from_secs(120), child.wait_with_output()).await {
        Ok(Ok(out)) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        Ok(Ok(out)) => Err(failed(&out.stderr)),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(failed(b"")),
    }
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// A repository's HEAD as `repoView` sees it: a blobless clone under
/// `plugins/.repos/`.
pub struct RepoView {
    pub dir: PathBuf,
    pub files: Vec<String>,
}

impl RepoView {
    /// `read(path)`: the file at HEAD, or `None`.
    pub async fn read(dir: PathBuf, files: &[String], p: String) -> Option<String> {
        if !files.contains(&p) {
            return None;
        }
        let out = git(&[s(&["-C"]), vec![dir.display().to_string()], s(&["show"]), vec![format!("HEAD:{p}")]].concat()).await.unwrap_or_default();
        (!out.is_empty()).then_some(out)
    }
}

pub async fn repo_view(m: &Market, url: &str) -> Result<RepoView, String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let tail = url.strip_prefix("https://github.com/").unwrap_or(url);
    let name = R.get_or_init(|| Regex::new(r"[^\w.-]+").unwrap()).replace_all(tail, "__").into_owned();
    let dir = m.plugins.join(".repos").join(name);
    if !dir.join(".git").exists() {
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(dir.parent().unwrap_or(&dir));
        git(&[s(&["clone", "-q", "--depth", "1", "--filter=blob:none", "--no-checkout", url]), vec![dir.display().to_string()]].concat()).await?;
    }
    let list = git(&[s(&["-C"]), vec![dir.display().to_string()], s(&["ls-tree", "-r", "--name-only", "HEAD"])].concat()).await?;
    Ok(RepoView { dir, files: list.split('\n').filter(|l| !l.is_empty()).map(str::to_owned).collect() })
}

/// What `fetchPlugin` records: the root (relative to `plugins/`) and the
/// skills `[{name, file}]`.
pub struct Fetched {
    pub root: String,
    pub skills: Vec<Value>,
}

/// `fetchPlugin(e)`: the plugin's directory (and catalog skills living
/// elsewhere in the repo) checked out into `<base>/<key>` (a review's
/// staging folder; install moves it to `plugins/<key>`). The recorded
/// paths are relative to that folder, as they will be to `plugins/`.
pub async fn fetch_plugin(m: &Market, e: &Value, base: &Path) -> Result<Fetched, String> {
    let hint = e.get("install").filter(|h| js::truthy(h)).cloned().unwrap_or(json!({}));
    let display = super::string(e.get("displayName"));
    if truthy(hint.get("unavailable")) {
        return Err(format!("{display} can't be installed: {}", js::string_of(&hint["unavailable"])));
    }
    let repo = match hint.get("repo").filter(|r| js::truthy(r)) {
        Some(r) => js::string_of(r),
        None => super::resolve::repo_of(e).ok_or_else(|| format!("{display} has no public repository"))?,
    };
    let v = repo_view(m, &repo).await?;
    let mut path: Option<String> = nn(hint.get("path")).map(js::string_of);
    let mut skills: Vec<String> = entries(nn(hint.get("skills"))).into_iter().map(|(_, f)| js::string_of(&f)).collect();
    let mut servers = hint.get("servers").cloned().unwrap_or(Value::Null);
    let has = |p: &str| p.is_empty() || v.files.iter().any(|f| f.starts_with(&format!("{p}/")));
    if path.as_deref().is_none_or(|p| !has(p)) {
        let (dir, files) = (v.dir.clone(), v.files.clone());
        let r = super::resolve::resolve_entry(e, &v.files, |p| RepoView::read(dir.clone(), &files, p)).await?;
        path = Some(r.path);
        skills = r.skills.into_iter().map(|(_, f)| f).collect();
        servers = Value::Object(r.mcp_servers);
    }
    let path = path.unwrap_or_default();
    let key = super::string(e.get("key"));
    let dest = base.join(&key);
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    // small plugin dirs come over whole; a big repo's root only brings its
    // plugin parts and what its MCP config references
    let pre = if path.is_empty() { String::new() } else { format!("{path}/") };
    let under: Vec<&String> = v.files.iter().filter(|f| f.starts_with(&pre)).collect();
    let mut specs: Vec<String> = Vec::new();
    fn add(specs: &mut Vec<String>, s: String) {
        if !specs.contains(&s) {
            specs.push(s);
        }
    }
    let whole = if path.is_empty() { ".".to_owned() } else { path.clone() };
    if under.len() <= 3000 {
        add(&mut specs, whole.clone());
    } else {
        static REF: OnceLock<Regex> = OnceLock::new();
        let top: Vec<String> = under.iter().map(|f| f[pre.len()..].split('/').next().unwrap_or("").to_owned()).collect();
        for p in [
            ".buwiz-plugin",
            ".cursor-plugin",
            ".claude-plugin",
            ".grok-plugin",
            ".codex-plugin",
            ".plugin",
            "plugin.json",
            "mcp.json",
            ".mcp.json",
            "skills",
            "commands",
            "agents",
            "hooks",
            "rules",
            "assets",
            ".lsp.json",
            "settings.json",
        ] {
            if top.iter().any(|t| t == p) {
                add(&mut specs, format!("{pre}{p}"));
            }
        }
        let refs = REF.get_or_init(|| Regex::new(r"\$\{(?:CLAUDE|CURSOR)_PLUGIN_ROOT\}/([\w./-]+)").unwrap());
        let text = js::stringify(if js::truthy(&servers) { &servers } else { &Value::Null });
        for c in refs.captures_iter(&text) {
            let first = c[1].split('/').next().unwrap_or("").to_owned();
            if top.contains(&first) {
                add(&mut specs, format!("{pre}{first}"));
            }
        }
        if specs.is_empty() {
            add(&mut specs, whole.clone());
        }
    }
    for f in &skills {
        let d = posix_dirname(f);
        if !path.is_empty() && !d.starts_with(&format!("{path}/")) {
            add(&mut specs, d);
        }
    }
    // blobs arrive lazily from the promisor remote: one more try
    let co = [s(&["-C"]), vec![v.dir.display().to_string(), format!("--work-tree={}", dest.display())], s(&["checkout", "-q", "HEAD", "--"]), specs].concat();
    if git(&co).await.is_err() {
        git(&co).await?;
    }
    Ok(Fetched {
        root: if path.is_empty() { key.clone() } else { super::posix_join(&key, &path) },
        skills: skills
            .iter()
            .map(|f| {
                let parts: Vec<&str> = f.split('/').collect();
                let mut o = Map::new();
                if parts.len() >= 2 {
                    o.insert("name".into(), json!(parts[parts.len() - 2]));
                }
                o.insert("file".into(), json!(super::posix_join(&key, f)));
                Value::Object(o)
            })
            .collect(),
    })
}

/// `installLegacy(p)`: a vendored plugin's own dir, the clone already in
/// `plugins/<name>`, else a clone of its repo at its pinned sha into
/// `<base>/<name>` (a review's staging folder).
pub async fn install_legacy(m: &Market, p: &Value, base: &Path) -> Result<PathBuf, String> {
    let name = super::string(p.get("name"));
    let src = p.get("source");
    let path = at(src, "path");
    if super::text(at(src, "type")) == Some("local") || (truthy(path) && !truthy(at(src, "url"))) {
        let rel = js::string_of(path.unwrap_or(&Value::Null));
        let root = super::join(&m.server.join("plugins"), rel.strip_prefix("./").unwrap_or(&rel));
        if !root.exists() {
            return Err(format!("vendored source missing for {name}"));
        }
        return Ok(root);
    }
    let installed = m.plugins.join(&name);
    let dir = if installed.join(".git").exists() { installed } else { base.join(&name) };
    if !dir.join(".git").exists() {
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(dir.parent().unwrap_or(&dir));
        let url = at(src, "url").filter(|u| js::truthy(u)).map(js::string_of).unwrap_or_default();
        git(&[s(&["clone", "--depth", "1", url.strip_suffix(".git").unwrap_or(&url)]), vec![dir.display().to_string()]].concat()).await?;
        if let Some(sha) = at(src, "sha").filter(|v| js::truthy(v)).map(js::string_of) {
            let d = dir.display().to_string();
            if git(&s(&["-C", &d, "fetch", "--depth", "1", "origin", &sha])).await.is_ok() {
                let _ = git(&s(&["-C", &d, "checkout", "-q", &sha])).await;
            }
        }
    }
    Ok(if truthy(path) { super::join(&dir, &js::string_of(path.unwrap())) } else { dir })
}

/// `removePluginFiles(key)`.
pub fn remove_plugin_files(m: &Market, key: &str) {
    let ok = !key.is_empty() && !key.starts_with('.') && key.chars().all(|c| c.is_ascii_alphanumeric() || "_.@-".contains(c));
    if ok {
        let _ = std::fs::remove_dir_all(m.plugins.join(key));
    }
}

/// plugins.mjs's `frontDesc` (`skillDescription`): a SKILL.md's
/// frontmatter description, folded or plain.
pub fn front_desc(head: &str) -> String {
    static FOLDED: OnceLock<Regex> = OnceLock::new();
    static PLAIN: OnceLock<Regex> = OnceLock::new();
    let folded = FOLDED.get_or_init(|| Regex::new(r"(?m)^description:\s*[>|]-?\s*\n((?:[ \t]+.+\n?)+)").unwrap());
    let plain = PLAIN.get_or_init(|| Regex::new(r#"(?m)^description:\s*["']?(.+?)["']?\s*$"#).unwrap());
    if let Some(c) = folded.captures(head) {
        let d = js::squeeze(&c[1]);
        if !d.is_empty() {
            return d;
        }
    }
    plain.captures(head).map(|c| c[1].trim_matches(js::is_space).to_owned()).unwrap_or_default()
}

fn read_head(p: &Path) -> Option<String> {
    let text = std::fs::read_to_string(p).ok()?;
    Some(js::slice_utf16(&text, 3000))
}

/// `pluginInventory(key, root)`.
pub fn plugin_inventory(m: &Market, ctx: &HubCtx, key: &str, root: &Path) -> Value {
    let meta = m.installed_meta(ctx, key).unwrap_or(json!({}));
    inventory_at(&m.plugins, root, meta.get("skills"), m.server_configs(ctx, key))
}

/// What the plugin at `root` ships: `skills` are the recorded `{name, file}`
/// (files relative to `base`), else its `skills/` folder.
pub fn inventory_at(base: &Path, root: &Path, skills: Option<&Value>, servers: Map<String, Value>) -> Value {
    let list_dir = |sub: &str, md: bool| -> Vec<Value> {
        let base = root.join(sub);
        let Ok(list) = crate::hub::pipeline::versions::read_dir_sorted(&base) else { return Vec::new() };
        list.into_iter()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.'))
            .map(|n| {
                let file = if md { base.join(&n) } else { base.join(&n).join("SKILL.md") };
                json!({ "name": n.strip_suffix(".md").unwrap_or(&n), "description": read_head(&file).map(|h| front_desc(&h)).unwrap_or_default() })
            })
            .collect()
    };
    let skills = match skills {
        Some(Value::Array(list)) if !list.is_empty() => {
            let mut seen: Vec<String> = Vec::new();
            list.iter()
                .filter(|s| {
                    let f = super::string(s.get("file"));
                    !seen.contains(&f) && {
                        seen.push(f);
                        true
                    }
                })
                .map(|s| {
                    let file = base.join(super::string(s.get("file")));
                    let mut o = Map::new();
                    if let Some(n) = s.get("name") {
                        o.insert("name".into(), n.clone());
                    }
                    o.insert("description".into(), json!(read_head(&file).map(|h| front_desc(&h)).unwrap_or_default()));
                    Value::Object(o)
                })
                .collect()
        }
        _ => list_dir("skills", false),
    };
    let manifest = manifest_at(root);
    let author = match manifest.get("author") {
        Some(a) if truthy(a.get("name")) => a["name"].clone(),
        Some(Value::String(s)) => json!(s),
        _ => Value::Null,
    };
    json!({
        "skills": skills,
        "commands": list_dir("commands", true),
        "agents": list_dir("agents", true),
        "hooks": root.join("hooks/hooks.json").exists(),
        "lsp": root.join(".lsp.json").exists(),
        "servers": Value::Object(servers),
        "manifest": { "author": author, "version": super::or(manifest.get("version"), Value::Null), "license": super::or(manifest.get("license"), Value::Null) },
    })
}

/// The plugin manifest at `root`: the first `plugin.json` with a name, `{}` with none.
pub fn manifest_at(root: &Path) -> Value {
    for f in [".buwiz-plugin/plugin.json", ".cursor-plugin/plugin.json", ".claude-plugin/plugin.json", ".grok-plugin/plugin.json", "plugin.json"] {
        let j = parse(std::fs::read_to_string(root.join(f)).ok().as_deref(), Value::Null);
        if truthy(j.get("name")) {
            return j;
        }
    }
    json!({})
}

/// `skillFile(key, skill)`: the SKILL.md of one of its skills.
pub fn skill_file(m: &Market, ctx: &HubCtx, key: &str, skill: &str) -> Option<PathBuf> {
    let meta = m.installed_meta(ctx, key).unwrap_or(json!({}));
    if let Some(hit) = meta.get("skills").and_then(Value::as_array).and_then(|l| l.iter().find(|s| s.get("name") == Some(&json!(skill)))) {
        return Some(m.plugins.join(super::string(hit.get("file"))));
    }
    let root = m.plugin_root(ctx, key)?;
    let ok = !skill.is_empty() && skill.chars().all(|c| c.is_ascii_alphanumeric() || "_.@-".contains(c));
    ok.then(|| root.join("skills").join(skill).join("SKILL.md"))
}

/// `pluginSkillsInject()`: installed plugin skills as the agent's index;
/// an enabled mini app's package skills (`v1_plugins.skills`) join it
/// under its short name.
pub fn plugin_skills_inject(m: &Market, ctx: &HubCtx) -> String {
    let names: Vec<String> = ctx
        .db
        .with(|db| {
            let mut st = db.prepare("SELECT name FROM installed_plugins ORDER BY created_at")?;
            let r = st.query_map([], |r| r.get::<_, String>(0))?;
            r.collect()
        })
        .unwrap_or_default();
    let mut lines = Vec::new();
    for name in names {
        let Some(root) = m.plugin_root(ctx, &name) else { continue };
        let inv = plugin_inventory(m, ctx, &name, &root);
        for (_, sk) in entries(inv.get("skills")) {
            let sname = super::string(sk.get("name"));
            if let Some(f) = skill_file(m, ctx, &name, &sname).filter(|f| f.exists()) {
                let desc = sk.get("description").filter(|d| js::truthy(d)).map(js::string_of).unwrap_or_default();
                lines.push(format!("- {name}/{sname}: {} (file: {})", js::slice_utf16(&desc, 160), f.display()));
            }
        }
    }
    let apps: Vec<(String, String)> = ctx
        .db
        .with(|db| {
            let mut st = db.prepare("SELECT short, skills FROM v1_plugins WHERE enabled=1 AND package IS NOT NULL ORDER BY created_at")?;
            let r = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))?;
            r.collect()
        })
        .unwrap_or_default();
    for (short, skills) in apps {
        for sk in parse(Some(&skills), json!([])).as_array().into_iter().flatten() {
            let (sname, file) = (super::string(sk.get("name")), super::string(sk.get("file")));
            if !sname.is_empty() && Path::new(&file).exists() {
                let desc = sk.get("description").filter(|d| js::truthy(d)).map(js::string_of).unwrap_or_default();
                lines.push(format!("- {short}/{sname}: {} (file: {file})", js::slice_utf16(&desc, 160)));
            }
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    js::slice_utf16(
        &format!(
            "\nPlugin skills (installed): when a request matches a skill's description, read its SKILL.md with shell_run (cat the file) and follow it; files it references sit beside it.\n{}",
            lines.join("\n")
        ),
        24000,
    )
}

// ————— connector rows —————

/// McpHost's status of a server (`statusOf`): idle when this broker hosts none.
pub fn mcp_status(ctx: &HubCtx, id: &str) -> Value {
    ctx.mcp.as_ref().map_or_else(|| json!({ "status": "idle", "tools": 0 }), |m| m.status(id))
}

/// `invalidate(id)`: drops its client.
pub async fn mcp_invalidate(ctx: &HubCtx, id: &str) {
    if let Some(m) = &ctx.mcp {
        m.invalidate(id).await;
    }
}

/// What `upsertConnector` answers: the row's id, whether it made it.
pub struct Upserted {
    pub id: String,
    pub created: bool,
}

/// `upsertConnector(key, server, {id})`: one connector row from the
/// plugin's config and setup values (never probed).
pub async fn upsert_connector(m: &Market, ctx: &HubCtx, key: &str, server: &str, id: Option<String>) -> Result<Upserted, String> {
    let mat = m.materialize(ctx, key, server, None).ok_or_else(|| format!("{key} declares no server {server}"))?;
    if !mat.runnable() {
        return Err(format!("{server} has no url or command"));
    }
    let row_id = id.unwrap_or_else(|| m.server_id_for(ctx, key, server));
    let exists: Option<Option<String>> = ctx.db.with(|db| db.query_row("SELECT name FROM mcp_servers WHERE id=?", [&row_id], |r| r.get::<_, Option<String>>(0)).optional())?;
    let id = match &exists {
        Some(name) if name.as_deref() != Some(server) => None,
        _ => Some(row_id),
    };
    let id = upsert_server(ctx, id, server, &mat).await?;
    Ok(Upserted { id, created: exists.is_none() })
}

/// The part of `mcpUpsertServer` a connector takes: no bot, never by the
/// agent, never probed. Omitted fields keep the row's; REPLACE carries the
/// sign-in, label and switches over.
async fn upsert_server(ctx: &HubCtx, id: Option<String>, name: &str, m: &Materialized) -> Result<String, String> {
    type Cur = (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<i64>);
    let cur: Option<Cur> = match &id {
        Some(id) => ctx.db.with(|db| {
            db.query_row("SELECT name,url,command,bot,auth,account,disabled_tools,agent_pending FROM mcp_servers WHERE id=?", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))
            })
            .optional()
        })?,
        None => None,
    };
    let trim = |s: &str| s.trim_matches(js::is_space).to_owned();
    let final_name = trim(name);
    let final_url = trim(m.url.as_deref().or(cur.as_ref().and_then(|c| c.1.as_deref())).unwrap_or(""));
    let final_command = trim(m.command.as_deref().or(cur.as_ref().and_then(|c| c.2.as_deref())).unwrap_or(""));
    if final_name.is_empty() {
        return Err("server needs a name".into());
    }
    if final_command.is_empty() && final_url.is_empty() {
        return Err("server needs a command or url".into());
    }
    if let Some(id) = &id {
        let ok = !id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if cur.is_none() && !ok {
            return Err(format!("server id {} must be [a-z0-9-]+", js::slice_utf16(id, 40)));
        }
    }
    let sid = match id {
        Some(id) => id,
        None => {
            let base: String = super::dashes(&final_name, "-").trim_matches('-').chars().take(24).collect();
            let base = if base.is_empty() { "srv".to_owned() } else { base };
            let mut s = base.clone();
            let mut i = 2;
            while ctx.db.with(|db| db.query_row("SELECT 1 FROM mcp_servers WHERE id=?", [&s], |_| Ok(())).optional())?.is_some() {
                s = format!("{base}-{i}");
                i += 1;
            }
            s
        }
    };
    let now = super::now(ctx);
    let args = js::stringify(&Value::Array(m.args.iter().map(|a| json!(a)).collect()));
    let env = js::stringify(&Value::Object(m.env.clone()));
    let headers = js::stringify(&Value::Object(m.headers.clone()));
    let oauth = (!m.oauth_client.is_null()).then(|| if let Value::String(s) = &m.oauth_client { s.clone() } else { js::stringify(&m.oauth_client) });
    let c = cur.as_ref();
    ctx.db.with(|db| {
        db.execute(
            "INSERT OR REPLACE INTO mcp_servers(id,name,command,args,env,enabled,bot,created_at,url,headers,oauth_client,transport,auth,account,disabled_tools,agent_pending) VALUES(?,?,?,?,?,COALESCE((SELECT enabled FROM mcp_servers WHERE id=?),?),?,COALESCE((SELECT created_at FROM mcp_servers WHERE id=?),?),?,?,?,?,?,?,?,?)",
            rusqlite::params![
                sid,
                js::slice_utf16(&final_name, 60),
                final_command,
                args,
                env,
                sid,
                1,
                c.and_then(|c| c.3.clone()),
                sid,
                now,
                (!final_url.is_empty()).then_some(&final_url),
                headers,
                oauth,
                m.transport,
                c.and_then(|c| c.4.clone()),
                c.and_then(|c| c.5.clone()),
                c.and_then(|c| c.6.clone()),
                c.and_then(|c| c.7),
            ],
        )
    })?;
    mcp_invalidate(ctx, &sid).await;
    Ok(sid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mkt_front_matter_descriptions() {
        assert_eq!(front_desc("---\nname: a\ndescription: The a skill.\n---\n"), "The a skill.");
        assert_eq!(front_desc("---\ndescription: \"quoted\"\n---\n"), "quoted");
        assert_eq!(front_desc("---\ndescription: >-\n  A helper that\n  spans lines\n---\n"), "A helper that spans lines");
        assert_eq!(front_desc("---\ndescription: |\n  Render docs.\n---\n"), "Render docs.");
        assert_eq!(front_desc("no front matter"), "");
    }
}
