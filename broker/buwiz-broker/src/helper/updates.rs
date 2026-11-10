//! Newer releases of the installed plugins. For each plugin with a
//! release source, GitHub's latest release is read and what names its
//! version is verified against the trust store (the signed `plugin.json`
//! of a package, the signed index of a bare binary); a version above the
//! installed one is kept with the plugin (`Host::set_update`) and the list
//! carries it as `update`. Nothing here downloads a binary or installs:
//! the person reviews the update and presses Install.
//!
//! A release's notes are a stranger's text: only up to three plain lines
//! of them are kept, cut short, with no markup and no URLs.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::plugins::{Host, Row};
use super::release::{self, Source};
use crate::v1::manifest;
use crate::v1::package::{self, Account};
use crate::v1::trust::Trust;

/// The most notes kept, and each one's length in characters.
pub const NOTES_MAX: usize = 3;
pub const NOTE_CHARS: usize = 140;
/// How often every plugin's source is asked.
pub const EVERY: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

/// A release above the installed version.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Found {
    pub version: String,
    /// the release's tag: what its review fetches
    pub tag: String,
    pub notes: Vec<String>,
    /// the release's page, https only ("" otherwise)
    pub url: String,
    /// what it asks for that the installed version doesn't, in words
    pub access: Vec<String>,
}

/// `https://github.com/<owner>/<repo>` of what an install or a
/// `plugin.json` names as its source (a release URL, a repository URL or
/// `owner/repo`); `None` for anything else.
pub fn repository(source: &str) -> Option<String> {
    let s = source.trim();
    if !s.contains('/') {
        return None;
    }
    release::parse_url(s).ok().map(|src| format!("https://github.com/{}/{}", src.owner, src.repo))
}

/// Where `row`'s releases are: the source its install recorded, else its
/// package's `homepage` when that is a GitHub repository, else, for an id
/// `app.buwiz.<slug>`, `buwiz-app/<slug>`.
pub fn source_of(row: &Row) -> Option<Source> {
    let named = |s: &str| repository(s).and_then(|r| release::parse_url(&r).ok());
    if let Some(src) = row.source.as_deref().and_then(named) {
        return Some(src);
    }
    let homepage = row.package.as_deref().and_then(|dir| std::fs::read_to_string(std::path::Path::new(dir).join(package::PLUGIN_FILE)).ok()).and_then(|text| {
        let v: Value = serde_json::from_str(&text).ok()?;
        v.get("homepage").and_then(Value::as_str).filter(|h| h.starts_with("https://github.com/")).and_then(named)
    });
    if homepage.is_some() {
        return homepage;
    }
    let slug = row.id.strip_prefix("app.buwiz.")?;
    release::parse_url(slug).ok().filter(|s| s.owner == release::FIRST_PARTY_ORG)
}

/// A line of release notes as plain text: its list or heading mark, its
/// emphasis and code marks gone, a link as its words, tags dropped.
fn plain_line(line: &str) -> String {
    let mut s = line.trim();
    s = s.trim_start_matches('#').trim_start();
    for mark in ["- [ ] ", "- [x] ", "- ", "* ", "+ ", "• "] {
        if let Some(rest) = s.strip_prefix(mark) {
            s = rest;
            break;
        }
    }
    let numbered = s.split_once(". ").filter(|(n, _)| !n.is_empty() && n.len() <= 3 && n.bytes().all(|b| b.is_ascii_digit()));
    if let Some((_, rest)) = numbered {
        s = rest;
    }
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            // <tag>: dropped whole
            '<' => match chars[i..].iter().position(|c| *c == '>') {
                Some(end) => i += end,
                None => break,
            },
            // [words](url) and ![alt](url): the words
            '[' => {
                let close = chars[i..].iter().position(|c| *c == ']').map(|p| i + p);
                match close {
                    Some(c) if chars.get(c + 1) == Some(&'(') => {
                        out.extend(&chars[i + 1..c]);
                        i = chars[c..].iter().position(|x| *x == ')').map_or(chars.len(), |p| c + p);
                    }
                    _ => out.push('['),
                }
            }
            '*' | '`' | '_' if chars[i] != '_' || i == 0 || chars.get(i + 1).is_none_or(|c| !c.is_alphanumeric()) || !chars[i - 1].is_alphanumeric() => {}
            '!' if chars.get(i + 1) == Some(&'[') => {}
            c => out.push(c),
        }
        i += 1;
    }
    // a bare URL says nothing a person reads, and is not shown
    let words: Vec<&str> = out.split_whitespace().filter(|w| !w.contains("://")).collect();
    words.join(" ")
}

/// Up to [`NOTES_MAX`] plain lines of a release's notes, each cut to
/// [`NOTE_CHARS`]. List items come first when the notes have any:
/// headings and prose around a change list are not the changes.
pub fn notes_of(body: &str) -> Vec<String> {
    let is_item = |l: &str| {
        let l = l.trim_start();
        ["- ", "* ", "+ ", "• "].iter().any(|m| l.starts_with(m))
    };
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with("```")).collect();
    let items: Vec<&str> = lines.iter().copied().filter(|l| is_item(l)).collect();
    let from = if items.is_empty() { lines.into_iter().filter(|l| !l.trim_start().starts_with('#')).collect() } else { items };
    from.into_iter()
        .map(plain_line)
        .filter(|l| l.chars().any(char::is_alphanumeric))
        .map(|l| if l.chars().count() > NOTE_CHARS { format!("{}…", l.chars().take(NOTE_CHARS - 1).collect::<String>().trim_end()) } else { l })
        .take(NOTES_MAX)
        .collect()
}

/// An account scope in words: Google's by product ("Google Drive",
/// "Gmail (read only)"), any other as it is written.
pub fn scope_words(scope: &str) -> String {
    let Some(name) = scope.strip_prefix("https://www.googleapis.com/auth/") else {
        return scope.chars().take(80).collect();
    };
    let read_only = name.ends_with(".readonly");
    let product = match name.split('.').next().unwrap_or_default() {
        "gmail" => "Gmail",
        "calendar" => "Google Calendar",
        "drive" => "Google Drive",
        "contacts" => "Google Contacts",
        "documents" => "Google Docs",
        "spreadsheets" => "Google Sheets",
        "presentations" => "Google Slides",
        "tasks" => "Google Tasks",
        "userinfo" => "your Google profile",
        other => return format!("Google {other}{}", if read_only { " (read only)" } else { "" }),
    };
    format!("{product}{}", if read_only { " (read only)" } else { "" })
}

/// The scopes `next`'s accounts ask for that `prev`'s don't, in words,
/// each said once. A product's read-only scope is no new access where its
/// wider scope is held already or asked for too.
pub fn added_scopes(prev: &[Account], next: &[Account]) -> Vec<String> {
    let had: HashSet<(&str, &str)> = prev.iter().flat_map(|a| a.scopes.iter().map(|s| (a.provider.as_str(), s.as_str()))).collect();
    let asks: HashSet<(&str, &str)> = next.iter().flat_map(|a| a.scopes.iter().map(|s| (a.provider.as_str(), s.as_str()))).collect();
    let covered = |provider: &str, scope: &str| {
        had.contains(&(provider, scope)) || scope.strip_suffix(".readonly").is_some_and(|wide| had.contains(&(provider, wide)) || asks.contains(&(provider, wide)))
    };
    let mut out: Vec<String> = Vec::new();
    for a in next {
        for s in a.scopes.iter().filter(|s| !covered(&a.provider, s)) {
            let words = scope_words(s);
            if !out.contains(&words) {
                out.push(words);
            }
        }
    }
    out
}

/// The latest release of `src` when it is `row`'s plugin at a version
/// above the installed one, verified against `trust`; `None` when the
/// installed version is the newest. `accounts` are the installed
/// package's.
pub async fn latest(api: &str, src: &Source, row: &Row, accounts: &[Account], trust: &Trust) -> Result<Option<Found>, String> {
    let plain = api.starts_with("http://");
    let url = format!("{}/repos/{}/{}/releases/latest", api.trim_end_matches('/'), src.owner, src.repo);
    let body = release::small(&url, "application/vnd.github+json", plain, release::SMALL_MAX).await?;
    let rel: Value = serde_json::from_slice(&body).map_err(|_| "GitHub's release answer is not JSON".to_owned())?;
    let asset = |name: &str, cap: usize| {
        let url = release::asset_url(&rel, name);
        async move { release::small(&url?, "application/octet-stream", plain, cap).await }
    };
    // the manifest of the new version, when it is a small asset of the release
    let manifest_of = |name: String, sha256: String| {
        let got = asset(&name, release::SMALL_MAX);
        async move {
            let bytes = got.await.ok()?;
            (buwiz_protocol::keys::sha256_hex(&bytes) == sha256).then(|| serde_json::from_slice::<Value>(&bytes).ok()).flatten()
        }
    };
    let (id, version, mut access, next) = if release::asset_url(&rel, release::PLUGIN_ASSET).is_ok() {
        let text = String::from_utf8(asset(release::PLUGIN_ASSET, package::PLUGIN_JSON_MAX).await?).map_err(|_| "plugin.json is not UTF-8".to_owned())?;
        let p = package::verify(&text, trust, manifest::HOST_PROTOCOL, &release::platform()).map_err(|e| e.message)?;
        if p.binary.is_none() {
            return Err(format!("no build for {}", release::platform()));
        }
        let signed = p.files.iter().find(|f| f.path == release::MANIFEST_FILE).map(|f| (f.asset.clone(), f.sha256.clone()));
        (p.id.clone(), p.version.clone(), added_scopes(accounts, &p.accounts), signed)
    } else {
        let text = asset(release::INDEX_ASSET, release::SMALL_MAX).await?;
        let ix = release::verify_index(&String::from_utf8_lossy(&text), trust, &release::platform())?;
        (ix.id, ix.version, Vec::new(), Some((ix.manifest.asset, ix.manifest.sha256)))
    };
    if id != row.id {
        return Err(format!("{}/{} releases {id}, not {}", src.owner, src.repo, row.id));
    }
    if manifest::compare_semver(&version, &row.version) != Some(1) {
        return Ok(None);
    }
    // the access it adds is part of what is announced: no manifest, no update
    if let Some((name, sha256)) = next {
        let m = manifest_of(name, sha256).await.filter(|m| m.get("id").and_then(Value::as_str) == Some(row.id.as_str()));
        let Some(m) = m else { return Err(format!("{version}'s manifest can't be read or doesn't match its signature")) };
        access.extend(manifest::added_permissions(&row.manifest(), &m));
    }
    let tag = rel.get("tag_name").and_then(Value::as_str).unwrap_or_default().to_owned();
    if tag.is_empty() {
        return Err("the release has no tag".into());
    }
    let url = rel.get("html_url").and_then(Value::as_str).filter(|u| u.starts_with("https://")).unwrap_or_default().to_owned();
    let notes = notes_of(rel.get("body").and_then(Value::as_str).unwrap_or_default());
    Ok(Some(Found { version, tag, notes, url, access }))
}

/// Asks every installed plugin's source for a newer release (`api`:
/// GitHub's API base) and keeps what it finds. A source that can't be
/// read leaves what was known and is logged once per plugin and reason.
/// → how many plugins' `update` changed.
pub async fn check_all(host: &Arc<Host>, api: &str) -> usize {
    static SAID: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let trust = host.trust();
    let mut changed = 0;
    for row in host.rows() {
        let Some(src) = source_of(&row) else { continue };
        let accounts = host.package_of(&row).map(|p| p.accounts).unwrap_or_default();
        match latest(api, &src, &row, &accounts, &trust).await {
            Ok(found) => {
                // the row as it is now: an install may have landed meanwhile
                if host.row(&row.id).is_some_and(|now| now.version == row.version) && host.set_update(&row.id, found.as_ref()) {
                    changed += 1;
                }
            }
            Err(e) => {
                let line = format!("{}: {}", row.id, crate::js::slice_utf16(&e, 200));
                if SAID.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).insert(line.clone()) {
                    eprintln!("buwiz-broker: update check: {line}");
                }
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_are_three_plain_lines_with_no_markup_and_no_urls() {
        let body = "## What's new\n\nSome prose first.\n\n- **Faster** search in `Mail` ([#31](https://github.com/o/r/pull/31))\n* Drafts keep their place <img src=x onerror=alert(1)>\n- See https://example.com/changes for more\n- A fourth\n";
        assert_eq!(notes_of(body), ["Faster search in Mail (#31)", "Drafts keep their place", "See for more"]);
        // no list: the prose, never a heading
        assert_eq!(notes_of("# 1.5.0\nCards survive restarts.\n\nSecond line.\n"), ["Cards survive restarts.", "Second line."]);
        assert_eq!(notes_of(""), Vec::<String>::new());
        assert_eq!(notes_of("- ---\n- ***\n"), Vec::<String>::new());
        let long = format!("- {}", "word ".repeat(60));
        let cut = &notes_of(&long)[0];
        assert!(cut.chars().count() <= NOTE_CHARS && cut.ends_with('…'), "{cut}");
        assert_eq!(notes_of("1. First\n2. snake_case stays\n"), ["First", "snake_case stays"]);
    }

    #[test]
    fn added_scopes_are_named_once_in_words() {
        let acct = |scopes: &[&str]| Account { id: "google".into(), provider: "google".into(), label: "Google account".into(), scopes: scopes.iter().map(|s| s.to_string()).collect(), required: true };
        let g = |s: &str| format!("https://www.googleapis.com/auth/{s}");
        let prev = [acct(&[&g("gmail.modify"), &g("calendar.events")])];
        let next = [acct(&[&g("gmail.modify"), &g("calendar.events"), &g("drive"), &g("drive.file"), &g("contacts.readonly"), "openid"])];
        assert_eq!(added_scopes(&prev, &next), ["Google Drive", "Google Contacts (read only)", "openid"]);
        assert!(added_scopes(&next, &prev).is_empty(), "a scope dropped is no new access");
        assert_eq!(scope_words(&g("cloud-platform")), "Google cloud-platform");
        // a read-only scope under a wider one held, or asked for with it, is no new access
        let held = [acct(&[&g("drive")])];
        assert!(added_scopes(&held, &[acct(&[&g("drive.readonly")])]).is_empty());
        assert_eq!(added_scopes(&prev, &[acct(&[&g("drive"), &g("drive.readonly")])]), ["Google Drive"]);
        assert_eq!(added_scopes(&held, &[acct(&[&g("gmail.readonly")])]), ["Gmail (read only)"]);
    }

    #[test]
    fn a_plugins_source_is_what_its_install_said_then_its_homepage_then_its_first_party_name() {
        let dir = std::env::temp_dir().join(format!("buwiz-updates-source-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let row = |id: &str, source: Option<&str>, package: bool| Row {
            id: id.into(),
            short: "x".into(),
            command: "x".into(),
            args: "[]".into(),
            env: "{}".into(),
            version: "1.0.0".into(),
            protocol: "1.0.0".into(),
            publisher_key_id: "k".into(),
            plugin_pub: "p".into(),
            manifest: "{}".into(),
            manifest_sha256: "0".repeat(64),
            tools: "[]".into(),
            enabled: true,
            disabled_tools: "[]".into(),
            created_at: 0,
            package: package.then(|| dir.display().to_string()),
            skills: "[]".into(),
            source: source.map(str::to_owned),
        };
        let at = |r: &Row| source_of(r).map(|s| format!("{}/{}", s.owner, s.repo));
        std::fs::write(dir.join(package::PLUGIN_FILE), r#"{"homepage": "https://github.com/acme/books"}"#).unwrap();
        assert_eq!(at(&row("com.acme.books", Some("https://github.com/fork/books"), true)).as_deref(), Some("fork/books"));
        assert_eq!(at(&row("com.acme.books", None, true)).as_deref(), Some("acme/books"), "installed before sources were recorded");
        std::fs::write(dir.join(package::PLUGIN_FILE), r#"{"homepage": "https://acme.example/books"}"#).unwrap();
        assert_eq!(at(&row("com.acme.books", None, true)), None, "a homepage elsewhere is no release source");
        assert_eq!(at(&row("app.buwiz.google-workspace", None, true)).as_deref(), Some("buwiz-app/google-workspace"));
        assert_eq!(at(&row("app.buwiz.word", None, false)).as_deref(), Some("buwiz-app/word"));
        assert_eq!(at(&row("com.acme.books", None, false)), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_source_is_a_github_repository_or_nothing() {
        assert_eq!(repository("https://github.com/buwiz-app/google-workspace/releases/tag/v1.5.0").as_deref(), Some("https://github.com/buwiz-app/google-workspace"));
        assert_eq!(repository("o/r").as_deref(), Some("https://github.com/o/r"));
        assert_eq!(repository("word"), None, "a bare name is not a recorded source");
        assert_eq!(repository("https://example.com/o/r"), None);
    }
}
