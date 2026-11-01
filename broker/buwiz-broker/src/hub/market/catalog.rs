//! plugins.mjs's catalog side: the first-party catalog (`plugins/catalog.json`,
//! possibly absent or empty), categories, an installed plugin the catalog
//! doesn't list (read from its installed record), setup values (with the
//! shared `@google` row), the setup fields a plugin shows, its MCP server
//! configs and their materialized rows' fields.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use regex_lite::Regex;
use rusqlite::OptionalExtension;
use serde_json::{json, Map, Value};

use super::{at, entries, nn, or, parse, spread, text, truthy};
use crate::hub::ctx::HubCtx;
use crate::js;

const CAT_ORDER: [(&str, &str); 11] = [
    ("credentials", "Password Managers"),
    ("productivity", "Productivity"),
    ("communication", "Communication"),
    ("design", "Design"),
    ("code", "Code"),
    ("data", "Data"),
    ("sales", "Sales"),
    ("marketing", "Marketing"),
    ("finance", "Finance"),
    ("research", "Research"),
    ("support", "Support"),
];

fn cat_of_key(k: &str) -> Option<&'static str> {
    Some(match k {
        "CREDENTIALS" | "LOGIN_AND_CREDENTIAL_MANAGEMENT" => "credentials",
        "PRODUCTIVITY" | "DOCUMENTS_AND_FILES" => "productivity",
        "COMMUNICATION" | "INBOX_AND_COLLABORATION" | "SCHEDULING" => "communication",
        "SALES" => "sales",
        "MARKETING" => "marketing",
        "SUPPORT" | "CUSTOMER_SUPPORT" => "support",
        "FINANCE" | "PAYMENTS" | "FINANCE_AND_LEGAL" => "finance",
        "DATA" | "DATA_ANALYTICS" => "data",
        "DESIGN" | "CANVAS" => "design",
        "CODE" | "INFRASTRUCTURE" => "code",
        "RESEARCH" => "research",
        _ => return None,
    })
}

/// The label of a category id.
pub fn cat_label(id: &str) -> Option<&'static str> {
    CAT_ORDER.iter().find(|(i, _)| *i == id).map(|(_, l)| *l)
}

/// `CATEGORIES`: `[{id, label}]`.
pub fn categories() -> Value {
    Value::Array(CAT_ORDER.iter().map(|(id, label)| json!({ "id": id, "label": label })).collect())
}

/// `keyNorm`.
fn key_norm(k: &str) -> String {
    let up = k.trim_matches(js::is_space).to_uppercase().replace('&', " AND ");
    let mut out = String::new();
    let mut gap = false;
    for c in up.chars() {
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            out.push(c);
            gap = false;
        } else if !gap {
            out.push('_');
            gap = true;
        }
    }
    out.trim_matches('_').to_owned()
}

fn cats_of(e: &Value) -> Vec<Value> {
    if truthy(e.get("marketplace")) {
        return Vec::new();
    }
    let keys: Vec<Value> = match nn(e.get("categoryKeys")) {
        Some(Value::Array(a)) => a.clone(),
        Some(other) => vec![other.clone()],
        None if truthy(e.get("categoryKey")) => vec![e["categoryKey"].clone()],
        None => vec![e.get("category").cloned().unwrap_or(Value::Null)],
    };
    super::unique(keys.iter().filter_map(|k| cat_of_key(&key_norm(&if js::truthy(k) { js::string_of(k) } else { String::new() }))).map(|c| json!(c)))
}

/// `titleCase`: dashes and underscores as spaces, each word capitalised.
pub fn title_case(n: &str) -> String {
    let mut out = String::new();
    let mut prev_word = false;
    let mut gap = false;
    for c in n.chars() {
        if c == '-' || c == '_' {
            if !gap {
                out.push(' ');
            }
            gap = true;
            prev_word = false;
            continue;
        }
        gap = false;
        let word = c.is_ascii_alphanumeric() || c == '_';
        if word && !prev_word {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        prev_word = word;
    }
    out
}

/// Per-plugin setup values (`@google` is the shared Google client).
pub const GOOGLE_GROUP: &str = "@google";

fn re(slot: &'static OnceLock<Regex>, p: &str) -> &'static Regex {
    slot.get_or_init(|| Regex::new(p).expect("a valid pattern"))
}

fn var_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"\$\{([^}]+)\}")
}

fn ident(s: &str) -> bool {
    let mut b = s.bytes();
    b.next().is_some_and(|c| c.is_ascii_alphabetic() || c == b'_') && b.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

// `${BUWIZ_FILES_DIR}`, and `${ROX_FILES_DIR}` (its old name) in configs
// written before
const BUILTIN: [&str; 9] = ["CLAUDE_PLUGIN_ROOT", "CURSOR_PLUGIN_ROOT", "PLUGIN_ROOT", "workspaceFolder", "userHome", "HOME", "USERPROFILE", "BUWIZ_FILES_DIR", "ROX_FILES_DIR"];

fn is_secret_key(k: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"(?i)KEY|TOKEN|SECRET|PASSWORD|PAT\b").is_match(k)
}

pub fn is_google_key(k: &str) -> bool {
    k == "GOOGLE_CLIENT_ID" || k == "GOOGLE_CLIENT_SECRET"
}

fn url_of(cfg: &Value) -> Option<&str> {
    cfg.get("url").and_then(Value::as_str)
}

fn is_google(cfg: &Value) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    url_of(cfg).is_some_and(|u| re(&R, r"^https://[a-z]+mcp\.googleapis\.com/").is_match(u))
}

fn uses_google_vars(cfg: &Value) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"\$\{GOOGLE_CLIENT_(ID|SECRET)\}").is_match(&js::stringify(cfg))
}

fn shares_google(cfg: &Value) -> bool {
    is_google(cfg) || uses_google_vars(cfg)
}

/// A connector pinning a vendor's own OAuth client id.
fn literal_client(cfg: &Value) -> bool {
    truthy(cfg.get("url")) && truthy(at(Some(cfg), "auth").and_then(|a| a.get("CLIENT_ID"))) && !js::string_of(&cfg["auth"]["CLIENT_ID"]).contains("${")
}

fn google_scope(url: &str) -> String {
    let host = url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_default();
    let scopes: &[&str] = match host.split('.').next().unwrap_or("") {
        "gmailmcp" => &["gmail.modify", "gmail.compose", "gmail.labels"],
        "calendarmcp" => &["calendar.events", "calendar.readonly"],
        "drivemcp" => &["drive.readonly", "drive.file"],
        "docsmcp" => &["documents", "drive.readonly"],
        "sheetsmcp" => &["spreadsheets", "drive.readonly"],
        "slidesmcp" => &["presentations", "drive.readonly"],
        _ => &[],
    };
    scopes.iter().map(|s| format!("https://www.googleapis.com/auth/{s}")).collect::<Vec<_>>().join(" ")
}

/// `encodeURIComponent`.
pub fn encode_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The catalog file: `plugins/catalog.json` beside Node's code, or the
/// file `BUWIZ_PLUGIN_CATALOG` names.
pub fn catalog_file(server: &Path) -> PathBuf {
    match buwiz_protocol::env::var("BUWIZ_PLUGIN_CATALOG") {
        Some(f) if !f.is_empty() => PathBuf::from(f),
        _ => server.join("plugins/catalog.json"),
    }
}

/// What the market reads once: the catalog file, and where installs go.
pub struct Market {
    /// Node's SERVER_DIR (server/src)
    pub server: PathBuf,
    /// `<data>/plugins`
    pub plugins: PathBuf,
    /// `<data>/generated/files` (`${BUWIZ_FILES_DIR}`, old name `${ROX_FILES_DIR}`)
    pub out_dir: PathBuf,
    /// the port Rox reaches (fixed OAuth redirects)
    pub port: u16,
    /// the catalog file's `version` (part of the catalog's rev)
    pub version: String,
    pub entries: Vec<Value>,
    by_key: HashMap<String, usize>,
    /// the entries listed by `source` (vendored, or a repo cloned whole)
    sourced: HashMap<String, Value>,
    /// dcrSupport's answers by server URL: `None` while unknown
    pub(crate) dcr: Arc<Mutex<HashMap<String, Option<bool>>>>,
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

impl Market {
    /// The catalog's entries: one with a `source` (vendored under
    /// `plugins/external_plugins/`, or a repository cloned whole) becomes
    /// a `legacy` entry; any other is a full entry (`key`, `install`
    /// hints, `fields`, …). No file, or no plugins in it, is an empty
    /// catalog.
    pub fn load(server: &Path, data: &Path, port: u16) -> Self {
        let file = read_json(&catalog_file(server)).unwrap_or_else(|| json!({ "plugins": [] }));
        let owner = at(file.get("owner"), "name").filter(|v| js::truthy(v)).map(js::string_of);
        let mut out: Vec<Value> = Vec::new();
        let mut sourced = HashMap::new();
        for p in file.get("plugins").and_then(Value::as_array).cloned().unwrap_or_default() {
            if !p.is_object() {
                continue;
            }
            let cats = Value::Array(cats_of(&p));
            if p.get("source").is_none() {
                let mut m = spread(Some(&p));
                m.insert("cats".into(), cats);
                out.push(Value::Object(m));
                continue;
            }
            let name = p.get("name").map(js::string_of).unwrap_or_else(|| "undefined".into());
            sourced.insert(name.clone(), p.clone());
            let repo = at(Some(&p), "source").and_then(|s| s.get("url")).and_then(Value::as_str).map(|u| u.strip_suffix(".git").unwrap_or(u).to_owned());
            let by = at(p.get("author"), "name").filter(|v| js::truthy(v)).map(js::string_of).or_else(|| owner.clone()).unwrap_or_default();
            out.push(json!({
                "key": name, "name": name, "pluginName": name, "displayName": or(p.get("displayName"), json!(title_case(&name))), "description": or(p.get("description"), json!("")),
                "category": or(p.get("category"), json!("other")), "cats": cats, "websiteUrl": or(p.get("homepage"), Value::Null),
                "repositoryUrl": repo.filter(|r| !r.is_empty()).map_or_else(|| or(p.get("repository"), Value::Null), Value::String),
                "connectors": [], "skills": [], "publisher": { "name": by, "displayName": by }, "legacy": true, "keywords": or(p.get("keywords"), json!([])),
            }));
        }
        // the first entry of a key wins
        let mut by_key = HashMap::new();
        for (i, e) in out.iter().enumerate() {
            by_key.entry(e.get("key").map(js::string_of).unwrap_or_else(|| "undefined".into())).or_insert(i);
        }
        Self {
            server: server.to_owned(),
            plugins: data.join("plugins"),
            out_dir: data.join("generated").join("files"),
            port,
            version: file.get("version").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default(),
            entries: out,
            by_key,
            sourced,
            dcr: Arc::default(),
        }
    }

    /// `entryOf(key)`: the catalog's entry.
    pub fn entry_of(&self, key: &str) -> Option<&Value> {
        self.by_key.get(key).map(|i| &self.entries[*i])
    }

    /// `legacyOf(key)`: the catalog's record of a plugin listed by source.
    pub fn legacy_of(&self, key: &str) -> Option<&Value> {
        self.sourced.get(key)
    }

    /// `entryFor(key)`: the catalog's entry, else (installed from a catalog
    /// that's gone, or no longer listed) one made from the installed
    /// record: its manifest's name and description, its connector rows.
    pub fn entry_for(&self, ctx: &HubCtx, key: &str) -> Option<Value> {
        if let Some(e) = self.entry_of(key) {
            return Some(e.clone());
        }
        let meta = self.installed_meta(ctx, key)?;
        Some(self.installed_entry(ctx, key, &meta))
    }

    /// The entry of an installed plugin the catalog doesn't list.
    pub fn installed_entry(&self, ctx: &HubCtx, key: &str, meta: &Value) -> Value {
        let manifest = self.plugin_root(ctx, key).map(|r| super::install::manifest_at(&r)).unwrap_or(Value::Null);
        let field = |k: &str| manifest.get(k).filter(|v| v.is_string() && js::truthy(v)).cloned();
        let author = match manifest.get("author") {
            Some(a) if truthy(a.get("name")) => js::string_of(&a["name"]),
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        let ids: Vec<String> = entries(at(Some(meta), "mcp")).into_iter().map(|(_, v)| js::string_of(&v)).collect();
        let mut connectors: Vec<Value> = Vec::new();
        for id in ids {
            let name = ctx.db.with(|db| db.query_row("SELECT name FROM mcp_servers WHERE id=?", [&id], |r| r.get::<_, Option<String>>(0)).optional()).ok().flatten().flatten();
            if let Some(n) = name.filter(|n| !n.is_empty()) {
                if !connectors.iter().any(|c| c["name"] == json!(n)) {
                    connectors.push(json!({ "name": n }));
                }
            }
        }
        json!({
            "key": key, "name": key, "pluginName": key, "displayName": field("displayName").unwrap_or_else(|| json!(title_case(key))),
            "description": field("description").unwrap_or(json!("")), "category": "other", "cats": [], "websiteUrl": field("homepage").unwrap_or(Value::Null),
            "repositoryUrl": or(at(Some(meta), "repo"), field("repository").unwrap_or(Value::Null)), "connectors": connectors, "skills": [],
            "publisher": { "name": author, "displayName": author }, "keywords": [], "uncataloged": true,
        })
    }

    /// `installedRow(key)`: its meta text (None: not installed).
    pub fn installed_row(&self, ctx: &HubCtx, key: &str) -> Option<Option<String>> {
        ctx.db.with(|db| db.query_row("SELECT meta FROM installed_plugins WHERE name=?", [key], |r| r.get::<_, Option<String>>(0)).optional()).ok().flatten()
    }

    /// `installedMeta(key)`.
    pub fn installed_meta(&self, ctx: &HubCtx, key: &str) -> Option<Value> {
        self.installed_row(ctx, key).map(|m| parse(m.as_deref(), json!({})))
    }

    /// `pluginRoot(key)`: where an installed plugin's files are.
    pub fn plugin_root(&self, ctx: &HubCtx, key: &str) -> Option<PathBuf> {
        let meta = self.installed_meta(ctx, key);
        if let Some(root) = nn(at(meta.as_ref(), "root")) {
            let r = super::join(&self.plugins, &js::string_of(root));
            return r.exists().then_some(r);
        }
        let Some(p) = self.legacy_of(key) else {
            // cloned whole from a catalog that no longer lists it: the clone
            // in plugins/<key> (a vendored one's files went with the catalog)
            let safe = !key.is_empty() && !key.starts_with('.') && key.chars().all(|c| c.is_ascii_alphanumeric() || "_.@-".contains(c));
            let dir = self.plugins.join(key);
            if meta.is_none() || !safe || !dir.join(".git").exists() {
                return None;
            }
            let path = text(at(meta.as_ref(), "path")).unwrap_or("");
            return Some(if path.is_empty() { dir } else { super::join(&dir, path) });
        };
        let src = p.get("source");
        let path = at(src, "path");
        if text(at(src, "type")) == Some("local") || (truthy(path) && !truthy(at(src, "url"))) {
            let rel = js::string_of(path.unwrap_or(&Value::Null));
            let root = super::join(&self.server.join("plugins"), rel.strip_prefix("./").unwrap_or(&rel));
            return root.exists().then_some(root);
        }
        let dir = self.plugins.join(js::string_of(p.get("name").unwrap_or(&Value::Null)));
        if !dir.exists() {
            return None;
        }
        Some(if truthy(path) { super::join(&dir, &js::string_of(path.unwrap())) } else { dir })
    }

    // ————— setup values —————

    fn raw_settings(&self, ctx: &HubCtx, key: &str) -> Value {
        let vals = ctx.db.with(|db| db.query_row("SELECT vals FROM plugin_settings WHERE plugin=?", [key], |r| r.get::<_, Option<String>>(0)).optional()).ok().flatten().flatten();
        parse(vals.as_deref(), json!({}))
    }

    /// `settingsOf(key)`: a Google plugin's own values with the shared ones over them.
    pub fn settings_of(&self, ctx: &HubCtx, key: &str) -> Map<String, Value> {
        let mut own = spread(Some(&self.raw_settings(ctx, key)));
        if key != GOOGLE_GROUP && self.plugin_uses_google(ctx, key) {
            for (k, v) in spread(Some(&self.raw_settings(ctx, GOOGLE_GROUP))) {
                own.insert(k, v);
            }
        }
        own
    }

    /// `saveSettings(key, vals)`: the valid non-blank values, trimmed; a
    /// Google plugin's Google keys to the shared row.
    pub fn save_settings(&self, ctx: &HubCtx, key: &str, vals: &Map<String, Value>) -> Result<Map<String, Value>, String> {
        static K: OnceLock<Regex> = OnceLock::new();
        let valid = re(&K, r"^[A-Za-z_][\w.]*$");
        let mut clean = Map::new();
        for (k, v) in entries(Some(&Value::Object(vals.clone()))) {
            let s = js::string_of(&v);
            if valid.is_match(&k) && !v.is_null() && !s.trim_matches(js::is_space).is_empty() {
                clean.insert(k, json!(s.trim_matches(js::is_space)));
            }
        }
        if key != GOOGLE_GROUP && self.plugin_uses_google(ctx, key) {
            let mut g = Map::new();
            for k in clean.keys().cloned().collect::<Vec<_>>() {
                if is_google_key(&k) {
                    g.insert(k.clone(), clean.shift_remove(&k).unwrap_or(Value::Null));
                }
            }
            self.save_settings(ctx, GOOGLE_GROUP, &g)?;
        }
        let now = super::now(ctx);
        ctx.db.with(|db| {
            if clean.is_empty() {
                db.execute("DELETE FROM plugin_settings WHERE plugin=?", [key]).map(|_| ())
            } else {
                db.execute("INSERT OR REPLACE INTO plugin_settings(plugin, vals, updated_at) VALUES(?,?,?)", rusqlite::params![key, js::stringify(&Value::Object(clean.clone())), now]).map(|_| ())
            }
        })?;
        Ok(clean)
    }

    // ————— server configs —————

    /// `serverConfigs(key)`: the install hints' servers, else the installed
    /// copy's manifests.
    pub fn server_configs(&self, ctx: &HubCtx, key: &str) -> Map<String, Value> {
        let e = self.entry_of(key);
        let hinted = at(at(e, "install"), "servers");
        if truthy(hinted) && !entries(hinted).is_empty() {
            return adjust_servers(hinted);
        }
        let Some(root) = self.plugin_root(ctx, key) else { return Map::new() };
        manifest_servers(&root)
    }

    /// `serverConfigs` of a plugin whose files are at `root` (not
    /// installed yet: a review's staging folder).
    pub fn server_configs_at(&self, key: &str, root: &Path) -> Map<String, Value> {
        let hinted = at(at(self.entry_of(key), "install"), "servers");
        if truthy(hinted) && !entries(hinted).is_empty() {
            return adjust_servers(hinted);
        }
        manifest_servers(root)
    }

    pub fn plugin_uses_google(&self, ctx: &HubCtx, key: &str) -> bool {
        self.server_configs(ctx, key).values().any(shares_google)
    }

    /// `fieldsOf(key)`: the catalog's fields, every `${KEY}` the configs use
    /// without one, then the OAuth client fields a connector needs.
    pub fn fields_of(&self, ctx: &HubCtx, key: &str) -> Vec<Value> {
        self.fields_with(ctx, key, &self.server_configs(ctx, key))
    }

    /// [`Market::fields_of`] over these server configs.
    pub fn fields_with(&self, ctx: &HubCtx, key: &str, servers: &Map<String, Value>) -> Vec<Value> {
        let e = self.entry_of(key);
        let mut out: Vec<Value> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for (_, f) in entries(at(e, "fields")) {
            let fo = Some(&f);
            seen.push(string_or_undefined(at(fo, "key")));
            let mut m = Map::new();
            if let Some(k) = at(fo, "key") {
                m.insert("key".into(), k.clone());
            }
            m.insert("label".into(), or(at(fo, "label"), at(fo, "key").cloned().unwrap_or(Value::Null)));
            m.insert("type".into(), or(at(fo, "type"), json!("string")));
            m.insert("placeholder".into(), or(at(fo, "placeholder"), at(fo, "key").cloned().unwrap_or(Value::Null)));
            m.insert("required".into(), json!(truthy(at(fo, "isRequired"))));
            m.insert("secret".into(), json!(truthy(at(fo, "isSecret"))));
            m.insert("hint".into(), or(at(fo, "hint"), json!("")));
            let options = entries(at(fo, "options"))
                .into_iter()
                .map(|(_, o)| {
                    let mut om = Map::new();
                    if let Some(l) = o.get("label") {
                        om.insert("label".into(), l.clone());
                    }
                    let raw = o.get("rawValue");
                    let parsed = match raw {
                        Some(Value::String(s)) => serde_json::from_str::<Value>(s).ok().filter(|v| !v.is_null()).unwrap_or_else(|| raw.cloned().unwrap()),
                        Some(v) => serde_json::from_str::<Value>(&js::string_of(v)).ok().filter(|v| !v.is_null()).unwrap_or_else(|| v.clone()),
                        None => Value::Null,
                    };
                    om.insert("value".into(), json!(if raw.is_none() { "undefined".to_owned() } else { js::string_of(&parsed) }));
                    Value::Object(om)
                })
                .collect();
            m.insert("options".into(), Value::Array(options));
            out.push(Value::Object(m));
        }
        for cfg in servers.values() {
            if literal_client(cfg) {
                continue;
            }
            let text = js::stringify(cfg);
            for c in var_re().captures_iter(&text) {
                let mut parts = c[1].split(":-");
                let (name, dflt) = (parts.next().unwrap_or("").to_owned(), parts.next().map(str::to_owned));
                if !ident(&name) || BUILTIN.contains(&name.as_str()) || seen.contains(&name) || is_google_key(&name) {
                    continue;
                }
                seen.push(name.clone());
                let label = abbreviations(&title_case(&name.to_lowercase()));
                let default = match &dflt {
                    Some(d) if !d.is_empty() => format!(" Defaults to `{d}` when left blank."),
                    _ => String::new(),
                };
                out.push(json!({
                    "key": name, "label": label, "type": "string", "placeholder": name, "required": false, "secret": is_secret_key(&name),
                    "hint": format!("Referenced as `${{{name}}}` in this plugin's MCP configuration.{default}"), "options": [],
                }));
            }
        }
        if servers.values().any(shares_google) {
            for f in [
                json!({ "key": "GOOGLE_CLIENT_ID", "label": "Google Client ID", "placeholder": "xxxxxxxx.apps.googleusercontent.com", "secret": false,
                    "hint": "Shared by every Google plugin (Gmail, Google Calendar, Drive, Docs, Sheets, Slides) — saving it here updates all of them. Create it in Google Cloud Console → APIs & Services → Credentials → OAuth client ID, type Desktop app." }),
                json!({ "key": "GOOGLE_CLIENT_SECRET", "label": "Google Client Secret", "placeholder": "GOCSPX-…", "secret": true,
                    "hint": "The Desktop app client's secret (Google issues one; it's sent on the token request). Shared by every Google plugin." }),
            ] {
                let k = f["key"].as_str().unwrap().to_owned();
                if !seen.contains(&k) {
                    seen.push(k);
                    let mut m = f.as_object().unwrap().clone();
                    for (k, v) in [("type", json!("string")), ("required", json!(true)), ("options", json!([])), ("oauth", json!(true)), ("shared", json!("google"))] {
                        m.insert(k.into(), v);
                    }
                    out.push(Value::Object(m));
                }
            }
        }
        let needs: Vec<(&String, &Value)> = servers.iter().filter(|(n, cfg)| !is_google(cfg) && self.needs_own_client(ctx, key, n, cfg)).collect();
        if !needs.is_empty() {
            let cb: Vec<String> = needs.iter().map(|(n, _)| crate::helper::oauth::fixed_redirect_url(self.port, &self.server_id_for(ctx, key, n))).collect();
            let mut add = |f: Value| {
                let k = f["key"].as_str().unwrap().to_owned();
                if !seen.contains(&k) {
                    seen.push(k);
                    out.push(f);
                }
            };
            add(json!({ "key": "CLIENT_ID", "label": "Client ID", "type": "string", "placeholder": "Your app's OAuth 2.0 Client ID", "required": true, "secret": false, "options": [], "oauth": true,
                "hint": format!("From your own developer app (OAuth 2.0, PKCE). Register {} as its callback URL.", cb.join(" and ")) }));
            add(
                json!({ "key": "CLIENT_SECRET", "label": "Client Secret", "type": "string", "placeholder": "Only for confidential (Web App) clients", "required": false, "secret": true, "options": [], "oauth": true,
                "hint": "Leave blank for a public (Native App) client — sign-in then uses PKCE only." }),
            );
            if needs.iter().any(|(_, cfg)| literal_client(cfg)) {
                add(json!({ "key": "BEARER_TOKEN", "label": "Bearer Token", "type": "string", "placeholder": "App-only bearer token", "required": false, "secret": true, "options": [], "oauth": true,
                    "hint": "Optional. Connects without signing in, sending Authorization: Bearer <token> (read-only app access). Signed-in OAuth tokens take precedence." }));
            }
        }
        out
    }

    /// `needsOwnClient(key, server, cfg)`.
    pub fn needs_own_client(&self, ctx: &HubCtx, key: &str, server: &str, cfg: &Value) -> bool {
        if !truthy(cfg.get("url")) || truthy(cfg.get("unavailable")) {
            return false;
        }
        if is_google(cfg) {
            return true;
        }
        if literal_client(cfg) {
            return self.dcr_support(url_of(cfg).unwrap_or("")) != Some(true);
        }
        let id = self.server_id_for(ctx, key, server);
        let auth = ctx.db.with(|db| db.query_row("SELECT auth FROM mcp_servers WHERE id=?", [&id], |r| r.get::<_, Option<String>>(0)).optional()).ok().flatten().flatten();
        truthy(parse(auth.as_deref(), json!({})).get("no_dcr"))
    }

    /// `serverIdFor(key, server)`: the row this plugin made, any row of
    /// that name, else the name slugged.
    pub fn server_id_for(&self, ctx: &HubCtx, key: &str, server: &str) -> String {
        let meta = self.installed_meta(ctx, key);
        let mine: Vec<Value> = at(meta.as_ref(), "mcp").and_then(Value::as_array).cloned().unwrap_or_default();
        let rows: Vec<String> = ctx
            .db
            .with(|db| {
                let mut st = db.prepare("SELECT id FROM mcp_servers WHERE name=? ORDER BY created_at")?;
                let r = st.query_map([server], |r| r.get::<_, String>(0))?;
                r.collect()
            })
            .unwrap_or_default();
        if let Some(id) = rows.iter().find(|id| mine.contains(&json!(id))) {
            return id.clone();
        }
        if let Some(id) = rows.first() {
            return id.clone();
        }
        let s = super::dashes(server, "-");
        let s: String = s.trim_matches('-').chars().take(24).collect();
        if s.is_empty() {
            "srv".into()
        } else {
            s
        }
    }

    /// `materialize(key, server, cfg)`: the setup values substituted into
    /// one server config.
    pub fn materialize(&self, ctx: &HubCtx, key: &str, server: &str, cfg: Option<&Value>) -> Option<Materialized> {
        let cfg = match cfg {
            Some(c) => c.clone(),
            None => self.server_configs(ctx, key).get(server)?.clone(),
        };
        if !js::truthy(&cfg) {
            return None;
        }
        let vals = self.settings_of(ctx, key);
        let root = self.plugin_root(ctx, key).map(|p| p.display().to_string()).unwrap_or_default();
        let fields = self.fields_of(ctx, key);
        let val = |k: &str| nn(vals.get(k)).map(js::string_of);
        let sub = |s: &Value| -> String {
            let s = js::string_of(s);
            var_re()
                .replace_all(&s, |c: &regex_lite::Captures| {
                    let (all, inner) = (&c[0], &c[1]);
                    if let Some(n) = inner.strip_prefix("env:") {
                        return val(n).or_else(|| std::env::var(n).ok()).unwrap_or_default();
                    }
                    let mut parts = inner.split(":-");
                    let (name, dflt) = (parts.next().unwrap_or(""), parts.next());
                    match name {
                        // 2.1 §1: ${PLUGIN_ROOT} for every plugin
                        "CLAUDE_PLUGIN_ROOT" | "CURSOR_PLUGIN_ROOT" | "PLUGIN_ROOT" => root.clone(),
                        "workspaceFolder" | "userHome" => val(name).unwrap_or_else(|| std::env::var("HOME").unwrap_or_default()),
                        "HOME" | "USERPROFILE" => val(name).or_else(|| std::env::var(name).ok()).unwrap_or_default(),
                        "BUWIZ_FILES_DIR" | "ROX_FILES_DIR" => self.out_dir.display().to_string(),
                        n if !ident(n) => all.to_owned(),
                        n => val(n).unwrap_or_else(|| dflt.unwrap_or("").to_owned()),
                    }
                })
                .into_owned()
        };
        let mut headers = Map::new();
        static BARE: OnceLock<Regex> = OnceLock::new();
        let bare = re(&BARE, r"(?i)^(Bearer|Basic|Token)\s*$");
        for (k, v) in entries(cfg.get("headers").filter(|h| js::truthy(h))) {
            let val = sub(&v).trim_matches(js::is_space).to_owned();
            if !val.is_empty() && !bare.is_match(&val) {
                headers.insert(k, json!(val));
            }
        }
        let own = self.needs_own_client(ctx, key, server, &cfg);
        let auth = cfg.get("auth");
        let scopes = at(auth, "scopes");
        let mut oauth_client = Value::Null;
        if is_google(&cfg) {
            if let Some(id) = val("GOOGLE_CLIENT_ID").filter(|s| !s.is_empty()) {
                let scope = match scopes {
                    Some(Value::Array(a)) => a.iter().map(|s| if s.is_null() { String::new() } else { js::string_of(s) }).collect::<Vec<_>>().join(" "),
                    _ => String::new(),
                };
                let mut m = Map::new();
                m.insert("client_id".into(), json!(id));
                if let Some(s) = val("GOOGLE_CLIENT_SECRET").filter(|s| !s.is_empty()) {
                    m.insert("client_secret".into(), json!(s));
                }
                m.insert("scope".into(), json!(if scope.is_empty() { google_scope(url_of(&cfg).unwrap_or("")) } else { scope }));
                m.insert("extra".into(), json!({ "access_type": "offline", "prompt": "consent" }));
                oauth_client = Value::Object(m);
            }
        } else if own || truthy(at(auth, "CLIENT_ID")) {
            let id = if own {
                val("CLIENT_ID").unwrap_or_default()
            } else if literal_client(&cfg) {
                String::new()
            } else {
                sub(&cfg["auth"]["CLIENT_ID"])
            };
            let secret = if own {
                val("CLIENT_SECRET").unwrap_or_default()
            } else if truthy(at(auth, "CLIENT_SECRET")) {
                sub(&cfg["auth"]["CLIENT_SECRET"])
            } else {
                String::new()
            };
            let scope = match scopes {
                Some(Value::Array(a)) => json!(a.iter().map(|s| if s.is_null() { String::new() } else { js::string_of(s) }).collect::<Vec<_>>().join(" ")),
                other => or(other, json!("")),
            };
            if !id.is_empty() {
                let mut m = Map::new();
                m.insert("client_id".into(), json!(id));
                if !secret.is_empty() {
                    m.insert("client_secret".into(), json!(secret));
                }
                if js::truthy(&scope) {
                    m.insert("scope".into(), scope);
                }
                oauth_client = Value::Object(m);
            }
        }
        let has_auth = |h: &Map<String, Value>| h.keys().any(|k| k.to_lowercase() == "authorization");
        if own && literal_client(&cfg) && val("BEARER_TOKEN").is_some_and(|t| !t.is_empty()) && !has_auth(&headers) {
            headers.insert("Authorization".into(), json!(format!("Bearer {}", val("BEARER_TOKEN").unwrap())));
        }
        let mut missing: Vec<String> = fields
            .iter()
            .filter(|f| truthy(f.get("required")) && !truthy(f.get("oauth")) && !truthy(vals.get(&string_or_undefined(f.get("key")))))
            .map(|f| f.get("label").map(js::string_of).unwrap_or_default())
            .collect();
        if shares_google(&cfg) {
            if !truthy(vals.get("GOOGLE_CLIENT_ID")) {
                missing.push("Google Client ID".into());
            }
            if !truthy(vals.get("GOOGLE_CLIENT_SECRET")) {
                missing.push("Google Client Secret".into());
            }
        } else if own && oauth_client.is_null() && !has_auth(&headers) {
            missing.push("Client ID".into());
        }
        let kind = if truthy(cfg.get("type")) { js::string_of(&cfg["type"]).to_lowercase() } else { String::new() };
        Some(Materialized {
            url: truthy(cfg.get("url")).then(|| sub(&cfg["url"])),
            command: truthy(cfg.get("command")).then(|| sub(&cfg["command"])),
            args: match cfg.get("args").filter(|a| js::truthy(a)) {
                Some(Value::Array(a)) => a.iter().map(&sub).collect(),
                _ => Vec::new(),
            },
            env: entries(cfg.get("env").filter(|e| js::truthy(e))).into_iter().map(|(k, v)| (k, sub(&v))).filter(|(_, v)| !v.is_empty()).map(|(k, v)| (k, json!(v))).collect(),
            headers,
            oauth_client,
            transport: (kind == "sse").then(|| "sse".to_owned()),
            missing,
            own,
        })
    }

    /// `dcrSupport(url)`: whether the server's authorization server takes
    /// dynamic client registration; `None` while unknown (the first ask
    /// starts the probe), cached for the process.
    pub fn dcr_support(&self, url: &str) -> Option<bool> {
        if url.is_empty() {
            return None;
        }
        let mut cache = self.dcr.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = cache.get(url) {
            return *v;
        }
        cache.insert(url.to_owned(), None);
        let (dcr, url) = (self.dcr.clone(), url.to_owned());
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                let v = dcr_probe(&url).await;
                dcr.lock().unwrap_or_else(|e| e.into_inner()).insert(url, v);
            });
        }
        None
    }
}

/// The servers a plugin's own manifests at `root` declare.
fn manifest_servers(root: &Path) -> Map<String, Value> {
    for m in [".mcp.json", "gemini-extension.json", "mcp.json"] {
        let j = parse(std::fs::read_to_string(root.join(m)).ok().as_deref(), Value::Null);
        let s = if j.is_null() { None } else { Some(or(j.get("mcpServers"), j.get("mcp").cloned().unwrap_or(Value::Null))) };
        if let Some(s) = s.filter(|s| s.is_object() || s.is_array()) {
            if !entries(Some(&s)).is_empty() {
                return adjust_servers(Some(&s));
            }
        }
    }
    Map::new()
}

/// What `materialize` makes of a server config.
#[derive(Clone, Debug, Default)]
pub struct Materialized {
    pub url: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: Map<String, Value>,
    pub headers: Map<String, Value>,
    pub oauth_client: Value,
    pub transport: Option<String>,
    pub missing: Vec<String>,
    pub own: bool,
}

impl Materialized {
    /// `!m.url && !m.command`
    pub fn runnable(&self) -> bool {
        self.url.as_deref().is_some_and(|s| !s.is_empty()) || self.command.as_deref().is_some_and(|s| !s.is_empty())
    }
}

fn string_or_undefined(v: Option<&Value>) -> String {
    v.map_or_else(|| "undefined".into(), js::string_of)
}

/// `.replace(/\b(Api|Mcp|Url|Id|Pat|Dd)\b/g, w => w.toUpperCase())`
fn abbreviations(s: &str) -> String {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"\b(Api|Mcp|Url|Id|Pat|Dd)\b").replace_all(s, |c: &regex_lite::Captures| c[0].to_uppercase()).into_owned()
}

/// plugin_resolve.mjs's `adjustServers`: Cursor's Google proxies go
/// straight to Google; servers behind Cursor's or Grok's gateways are
/// marked unavailable.
pub fn adjust_servers(servers: Option<&Value>) -> Map<String, Value> {
    static PROXY: OnceLock<Regex> = OnceLock::new();
    let proxy = re(&PROXY, r"^https://api\.cursor\.com/rest-mcp/google-(drive|sheets|docs|slides)/mcp/?$");
    let hosted = [
        ("https://api.cursor.com/", "Hosted by Cursor — it signs in through a Cursor account, which Buwiz Bot doesn't have"),
        ("https://connectors-gateway.grok.com/", "Hosted by Grok's connector gateway — it needs a Grok account session"),
    ];
    let mut out = Map::new();
    for (n, cfg) in entries(servers) {
        let mut c = cfg;
        if let Some(g) = url_of(&c).and_then(|u| proxy.captures(u)).map(|g| g[1].to_owned()) {
            let mut m = spread(Some(&c));
            m.insert("url".into(), json!(format!("https://{g}mcp.googleapis.com/mcp/v1")));
            c = Value::Object(m);
        }
        if let Some((_, why)) = url_of(&c).and_then(|u| hosted.iter().find(|(p, _)| u.starts_with(p))) {
            let mut m = spread(Some(&c));
            m.insert("unavailable".into(), json!(why));
            c = Value::Object(m);
        }
        out.insert(n, c);
    }
    out
}

/// The DCR probe (RFC 9728, then RFC 8414 metadata). Like Node's fetch with
/// NODE_USE_ENV_PROXY=1, a proxy in HTTPS_PROXY / HTTP_PROXY takes the
/// request; it is only asked to CONNECT (no TLS through it here), so a
/// proxied probe answers unknown.
async fn dcr_probe(url: &str) -> Option<bool> {
    let server = url::Url::parse(url).ok()?;
    if let Some(proxy) = proxy_for(&server) {
        let _ = connect_through(&proxy, &server).await;
        return None;
    }
    let f = crate::helper::oauth::Fetcher { init: Vec::new() };
    let mut issuer = server.join("/").ok()?;
    if let Ok(prm) = crate::helper::oauth::discover_prm(&f, &server, None).await {
        if let Some(Value::String(first)) = prm.get("authorization_servers").and_then(|a| a.get(0)) {
            if let Ok(u) = url::Url::parse(first) {
                issuer = u;
            }
        }
    }
    match crate::helper::oauth::discover_as(&f, &issuer).await {
        Ok(Some(md)) => Some(md.get("registration_endpoint").is_some_and(js::truthy)),
        _ => None,
    }
}

/// The proxy Node's fetch would use for `url` under NODE_USE_ENV_PROXY=1.
fn proxy_for(url: &url::Url) -> Option<String> {
    if std::env::var("NODE_USE_ENV_PROXY").as_deref() != Ok("1") {
        return None;
    }
    let var = |names: &[&str]| names.iter().find_map(|n| std::env::var(n).ok().filter(|v| !v.is_empty()));
    let proxy = if url.scheme() == "https" { var(&["HTTPS_PROXY", "https_proxy"]) } else { var(&["HTTP_PROXY", "http_proxy"]) }?;
    let host = url.host_str().unwrap_or("");
    let skip = var(&["NO_PROXY", "no_proxy"]).unwrap_or_default();
    if skip.split(',').map(str::trim).any(|h| !h.is_empty() && (h == "*" || host == h || host.ends_with(&format!(".{}", h.trim_start_matches('.'))))) {
        return None;
    }
    Some(proxy)
}

async fn connect_through(proxy: &str, target: &url::Url) -> Result<(), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let p = url::Url::parse(proxy).map_err(|e| e.to_string())?;
    let addr = format!("{}:{}", p.host_str().unwrap_or("127.0.0.1"), p.port_or_known_default().unwrap_or(80));
    let mut s = tokio::time::timeout(std::time::Duration::from_secs(10), tokio::net::TcpStream::connect(addr)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    let hp = format!("{}:{}", target.host_str().unwrap_or(""), target.port_or_known_default().unwrap_or(443));
    s.write_all(format!("CONNECT {hp} HTTP/1.1\r\nhost: {hp}\r\n\r\n").as_bytes()).await.map_err(|e| e.to_string())?;
    let mut buf = [0u8; 256];
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), s.read(&mut buf)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mkt_title_case_key_norm_and_labels() {
        assert_eq!(title_case("axiom"), "Axiom");
        assert_eq!(title_case("google-workspace"), "Google Workspace");
        assert_eq!(title_case("a__b-c"), "A B C");
        assert_eq!(key_norm(" Finance & Legal "), "FINANCE_AND_LEGAL");
        assert_eq!(abbreviations(&title_case(&"TEMPORAL_API_KEY".to_lowercase())), "Temporal API Key");
        assert_eq!(encode_component("a b/c"), "a%20b%2Fc");
        assert!(is_secret_key("TEMPORAL_API_KEY") && !is_secret_key("TEMPORAL_ADDRESS") && is_secret_key("my_pat"));
    }

    #[test]
    fn mkt_adjust_servers_rewrites_cursors_google_proxies_and_marks_gateways() {
        let s = json!({ "d": { "url": "https://api.cursor.com/rest-mcp/google-drive/mcp", "type": "http" }, "c": { "url": "https://api.cursor.com/x" }, "k": { "command": "x" } });
        assert_eq!(
            Value::Object(adjust_servers(Some(&s))),
            json!({ "d": { "url": "https://drivemcp.googleapis.com/mcp/v1", "type": "http" }, "c": { "url": "https://api.cursor.com/x", "unavailable": "Hosted by Cursor — it signs in through a Cursor account, which Buwiz Bot doesn't have" }, "k": { "command": "x" } })
        );
    }
}
