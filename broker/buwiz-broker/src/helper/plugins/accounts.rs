//! A mini app's accounts (§20): declared in its
//! package's `buwiz.accounts`, signed in from the mini app page, recorded
//! in `v1_accounts` (no tokens: the reference only) with the tokens in the
//! keystore, and pushed to the plugin (D4): `buwiz.account.set` after
//! provision, after sign-in and after each refresh, `buwiz.account.clear`
//! after sign-out, removal or a failed refresh. The broker refreshes 5
//! minutes before `expires_at` while the plugin runs. A token never
//! reaches the model, the chat, a log or the app.
//!
//! Providers: `google` (Google's endpoints, the shared `@google` client of
//! `plugin_settings`, `access_type=offline`, `prompt=consent`) and
//! `oauth:<issuer>` (RFC 8414 discovery, RFC 7591 registration per
//! sign-in). Both are PKCE S256 in the system browser with the front
//! door's loopback redirect, `/oauth/callback/account:<plugin>/<account>/<label>`.
//!
//! Several labels of an account can be signed in; one is active (`active`
//! in `v1_accounts`, else `default`, else the first connected): only it is
//! pushed with `buwiz.account.set`, the others are cleared in the plugin
//! while their tokens stay fresh in the keystore. A new sign-in is active;
//! `…/accounts/<account>/use {label}` switches; signing out the active one
//! makes another connected label active.
//!
//! Setup (`setup_view`, `save_setup`): what a provider needs before its
//! sign-in can start, as fields the user fills in. `google` needs the shared
//! client (`hub::market::catalog::google_fields`, saved in `@google` where
//! the legacy Google plugins read it too); `oauth:` registers itself and
//! needs none. The views say only whether a value is set, never a secret
//! (a field that is not secret, the Client ID, shows its value so it can be
//! changed). A changed or removed value expires every signed-in account of
//! that provider (its tokens were the old client's); a sign-in the provider
//! refuses for its client (`invalid_client`, `unauthorized_client`) says so
//! on the account's row, `fix: "setup"`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Map, Value};
use tokio::sync::broadcast;
use url::Url;

use super::{refuse, Fail, Host, Res, Row};
use crate::helper::keystore::label_ok;
use crate::helper::mcp::{self, Conn};
use crate::helper::oauth::{self, Fetcher};
use crate::js;
use crate::v1::package::{self, Account, Package};
use crate::v1::{manifest, V1Error};

pub(super) const SCHEMA: &str = "
  CREATE TABLE IF NOT EXISTS v1_accounts(
    plugin TEXT NOT NULL, account TEXT NOT NULL, label TEXT NOT NULL, provider TEXT NOT NULL,
    subject TEXT NOT NULL DEFAULT '', token_ref TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'none',
    scopes TEXT NOT NULL DEFAULT '[]', updated_at INTEGER NOT NULL, active INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(plugin, account, label));
";

/// `v1_accounts` from before `active`: the column added (no label active,
/// so `default` or the first connected one is).
pub(super) fn migrate(db: &rusqlite::Connection) -> rusqlite::Result<()> {
    let has: i64 = db.query_row("SELECT count(*) FROM pragma_table_info('v1_accounts') WHERE name='active'", [], |r| r.get(0))?;
    if has == 0 {
        db.execute_batch("ALTER TABLE v1_accounts ADD COLUMN active INTEGER NOT NULL DEFAULT 0")?;
    }
    Ok(())
}

pub const GOOGLE_AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const GOOGLE_TOKEN: &str = "https://oauth2.googleapis.com/token";
use crate::hub::market::catalog::{google_fields, GOOGLE_GROUP};
pub const DEFAULT_LABEL: &str = "default";
pub const NEEDS_GOOGLE_CLIENT: &str = "Add your Google client in Setup";
pub const GOOGLE_REFUSED_CLIENT: &str = "Google refused this client. Check the Client ID and Secret.";
const REFRESH_EARLY_MS: f64 = 5.0 * 60.0 * 1000.0;
const PENDING_FOR: Duration = Duration::from_secs(10 * 60);
pub const SET: &str = "buwiz.account.set";
pub const CLEAR: &str = "buwiz.account.clear";

/// Where sign-in goes: the redirect's port (the front door's), Google's
/// endpoints, how the browser opens. Tests swap all of them.
#[derive(Clone)]
pub struct AccountsConfig {
    pub port: u16,
    pub google_authorize: String,
    pub google_token: String,
    pub open: Arc<dyn Fn(&str) + Send + Sync>,
}

impl Default for AccountsConfig {
    fn default() -> Self {
        Self { port: 8787, google_authorize: GOOGLE_AUTHORIZE.into(), google_token: GOOGLE_TOKEN.into(), open: Arc::new(oauth::open_url) }
    }
}

/// A sign-in waiting for its callback.
#[derive(Clone)]
struct Pending {
    plugin: String,
    account: String,
    label: String,
    provider: String,
    state: String,
    verifier: String,
    redirect: String,
    token_endpoint: String,
    auth_methods: Vec<String>,
    client: Map<String, Value>,
    scopes: Vec<String>,
    until: Instant,
}

/// The accounts' share of the host.
pub(super) struct Accounts {
    cfg: Mutex<AccountsConfig>,
    pending: Mutex<HashMap<String, Pending>>,
    timers: Mutex<HashMap<String, u64>>,
    /// The last failed sign-in of each token ref: (why, the setup fixes it).
    errors: Mutex<HashMap<String, (String, bool)>>,
    events: broadcast::Sender<Value>,
}

impl Accounts {
    pub(super) fn new() -> Self {
        Self { cfg: Mutex::new(AccountsConfig::default()), pending: Mutex::new(HashMap::new()), timers: Mutex::new(HashMap::new()), errors: Mutex::new(HashMap::new()), events: broadcast::channel(256).0 }
    }

    fn cfg(&self) -> AccountsConfig {
        self.cfg.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn errors(&self) -> std::sync::MutexGuard<'_, HashMap<String, (String, bool)>> {
        self.errors.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<String, Pending>> {
        let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        p.retain(|_, x| x.until > Instant::now());
        p
    }
}

/// A `v1_accounts` row.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountRow {
    pub plugin: String,
    pub account: String,
    pub label: String,
    pub provider: String,
    pub subject: String,
    pub token_ref: String,
    pub state: String,
    pub scopes: Vec<String>,
    pub updated_at: i64,
    /// Marked the active label of its account ([`Host::active_label`]).
    pub active: bool,
}

impl AccountRow {
    fn from_sql(r: &rusqlite::Row) -> rusqlite::Result<AccountRow> {
        let scopes: String = r.get("scopes")?;
        Ok(AccountRow {
            plugin: r.get("plugin")?,
            account: r.get("account")?,
            label: r.get("label")?,
            provider: r.get("provider")?,
            subject: r.get("subject")?,
            token_ref: r.get("token_ref")?,
            state: r.get("state")?,
            scopes: serde_json::from_str(&scopes).unwrap_or_default(),
            updated_at: r.get::<_, f64>("updated_at")? as i64,
            active: r.get::<_, i64>("active").unwrap_or(0) != 0,
        })
    }
}

/// `<plugin>/<account>/<label>`: the token's keystore name.
pub fn token_ref(plugin: &str, account: &str, label: &str) -> String {
    format!("{plugin}/{account}/{label}")
}

/// The callback id a sign-in's redirect names.
pub fn flow_id(plugin: &str, account: &str, label: &str) -> String {
    format!("account:{}", token_ref(plugin, account, label))
}

fn label_of(b: &Value) -> Res<String> {
    match b.get("label") {
        None | Some(Value::Null) => Ok(DEFAULT_LABEL.into()),
        Some(Value::String(l)) if l.is_empty() => Ok(DEFAULT_LABEL.into()),
        Some(Value::String(l)) if label_ok(l) => Ok(l.clone()),
        _ => V1Error::err("invalid_args", "label must be 1-60 of A-Z a-z 0-9 . _ @ + - (a letter or digit first)"),
    }
}

/// The signed-in user an ID token names (its email, else its subject);
/// the token came from the token endpoint itself, so it is not verified
/// again.
fn subject_of(id_token: Option<&Value>) -> String {
    let payload = id_token.and_then(Value::as_str).and_then(|t| t.split('.').nth(1)).and_then(|p| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(p.trim_end_matches('=')).ok());
    let claims: Value = payload.and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    claims.get("email").or(claims.get("sub")).and_then(Value::as_str).map(|s| js::slice_utf16(s, 200)).unwrap_or_default()
}

fn scopes_of(s: Option<&Value>) -> Option<Vec<String>> {
    s.and_then(Value::as_str).map(|s| s.split_whitespace().map(str::to_owned).collect()).filter(|v: &Vec<String>| !v.is_empty())
}

fn random_state() -> String {
    buwiz_protocol::keys::random::<16>().iter().map(|b| format!("{b:02x}")).collect()
}

/// The setup `provider`'s sign-in needs: the `plugin_settings` row it is
/// saved in and its fields; `None` for a provider that needs none.
fn provider_setup(provider: &str) -> Option<(&'static str, Vec<Value>)> {
    (provider == "google").then(|| (GOOGLE_GROUP, google_fields().into_iter().collect()))
}

/// Whether `v` matches `pattern` (a field's own; a bad one never does).
fn matches(pattern: &str, v: &str) -> bool {
    regex_lite::Regex::new(pattern).is_ok_and(|r| r.is_match(v))
}

/// Why `v` (trimmed, not empty) fails field `f`'s shape check, if it does.
pub fn shape_error(f: &Value, v: &str) -> Option<String> {
    let s = |k: &str| f.get(k).and_then(Value::as_str);
    if s("pattern").is_some_and(|p| !matches(p, v)) {
        return Some(s("pattern_hint").unwrap_or("This value has the wrong shape").to_owned());
    }
    if s("not_pattern").is_some_and(|p| matches(p, v)) {
        return Some(s("not_pattern_hint").unwrap_or("This value has the wrong shape").to_owned());
    }
    None
}

/// `values` (key → trimmed value, as they would be stored) entered the
/// wrong way round: a field with `swap_with` whose value has the other
/// field's shape while the other's fails it. → its `swap_hint`.
pub fn swapped(fields: &[Value], values: &Map<String, Value>) -> Option<String> {
    let val = |k: &str| values.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
    for f in fields {
        let Some(other) = f.get("swap_with").and_then(Value::as_str) else { continue };
        let Some(o) = fields.iter().find(|x| x["key"] == json!(other)) else { continue };
        let (Some(mine), Some(theirs)) = (val(&js::string_of(&f["key"])), val(other)) else { continue };
        let Some(p) = o.get("pattern").and_then(Value::as_str) else { continue };
        if matches(p, mine) && !matches(p, theirs) {
            return Some(f.get("swap_hint").and_then(Value::as_str).unwrap_or("These two values look swapped").to_owned());
        }
    }
    None
}

fn is_set(vals: &Map<String, Value>, key: &str) -> bool {
    vals.get(key).and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty())
}

/// A failed token request: why, and whether the provider refused the
/// client itself (a wrong Client ID or Secret).
struct Refused {
    why: String,
    bad_client: bool,
}

impl From<Refused> for String {
    fn from(r: Refused) -> String {
        r.why
    }
}

/// `buwiz.account.set`'s arguments.
fn set_args(r: &AccountRow, t: &Value) -> Value {
    json!({
        "account": r.account, "label": r.label, "provider": r.provider, "subject": r.subject, "scopes": r.scopes,
        "access_token": t.get("access_token"), "token_type": t.get("token_type").filter(|x| js::truthy(x)).cloned().unwrap_or(json!("Bearer")),
        "expires_at": t.get("expires_at"),
    })
}

impl Host {
    /// The redirect port, Google's endpoints and the browser (tests).
    pub fn set_accounts_config(&self, cfg: AccountsConfig) {
        *self.accounts.cfg.lock().unwrap_or_else(|e| e.into_inner()) = cfg;
    }

    /// `{plugin, account, label, state}` each time an account changes.
    pub fn account_events(&self) -> broadcast::Receiver<Value> {
        self.accounts.events.subscribe()
    }

    /// The installed package of a record (its plugin.json read again,
    /// shape only: the gate verified it).
    pub fn package_of(&self, row: &Row) -> Option<Package> {
        let dir = row.package.as_deref()?;
        let text = std::fs::read_to_string(std::path::Path::new(dir).join(package::PLUGIN_FILE)).ok()?;
        package::parse(&text, manifest::HOST_PROTOCOL, &crate::helper::release::platform()).ok()
    }

    /// The accounts a plugin's package declares.
    pub fn declared_accounts(&self, id: &str) -> Vec<Account> {
        self.row(id).and_then(|r| self.package_of(&r)).map(|p| p.accounts).unwrap_or_default()
    }

    pub fn account_rows(&self, plugin: &str) -> Vec<AccountRow> {
        self.sql(|db| {
            let mut st = db.prepare("SELECT * FROM v1_accounts WHERE plugin=? ORDER BY account, label")?;
            let rows = st.query_map([plugin], AccountRow::from_sql)?.collect::<rusqlite::Result<Vec<_>>>();
            rows
        })
        .unwrap_or_default()
    }

    fn account_row(&self, plugin: &str, account: &str, label: &str) -> Option<AccountRow> {
        self.sql(|db| db.query_row("SELECT * FROM v1_accounts WHERE plugin=? AND account=? AND label=?", [plugin, account, label], AccountRow::from_sql).optional()).ok().flatten()
    }

    fn put_account(&self, r: &AccountRow) -> Res<()> {
        let now = self.clock.now() as i64;
        self.sql(|db| {
            db.execute(
                "INSERT OR REPLACE INTO v1_accounts(plugin,account,label,provider,subject,token_ref,state,scopes,updated_at,active) VALUES(?,?,?,?,?,?,?,?,?,?)",
                params![r.plugin, r.account, r.label, r.provider, r.subject, r.token_ref, r.state, js::stringify(&json!(r.scopes)), now, r.active as i64],
            )
        })?;
        Ok(())
    }

    /// The active label of `plugin`'s `account`: the connected one marked
    /// active, else `default` when connected, else the first connected.
    pub fn active_label(&self, plugin: &str, account: &str) -> Option<String> {
        let rows: Vec<AccountRow> = self.account_rows(plugin).into_iter().filter(|r| r.account == account && r.state == "connected").collect();
        let pick = rows.iter().find(|r| r.active).or_else(|| rows.iter().find(|r| r.label == DEFAULT_LABEL)).or(rows.first());
        pick.map(|r| r.label.clone())
    }

    /// Marks `label` the active one of `plugin`'s `account` (the others not).
    fn set_active(&self, plugin: &str, account: &str, label: &str) {
        let _ = self.sql(|db| db.execute("UPDATE v1_accounts SET active=(label=?) WHERE plugin=? AND account=?", [label, plugin, account]));
    }

    /// Pushes `r`'s tokens to the running plugin (refreshed first when due;
    /// a failed refresh: expired) and keeps them fresh. The caller holds the
    /// plugin's lock.
    async fn push_label(&self, r: &AccountRow) {
        let Some(mut t) = self.tokens(&r.token_ref).await else { return self.set_account_state(r, "expired") };
        if self.due(&t) {
            match self.refresh_account(r).await {
                Ok(fresh) => t = fresh,
                Err(_) => return self.set_account_state(r, "expired"),
            }
        }
        self.push_live(&r.plugin, SET, set_args(r, &t)).await;
        self.schedule_refresh(r, &t);
    }

    /// `POST …/accounts/<account>/use {label}`: `label` (signed in) becomes
    /// the active one: pushed, and the one before cleared in the plugin.
    pub async fn account_use(&self, plugin: &str, account: &str, b: &Value) -> Res {
        self.declared_one(plugin, account)?;
        let label = label_of(b)?;
        let Some(r) = self.account_row(plugin, account, &label).filter(|r| r.state == "connected") else {
            return V1Error::err("not_found", format!("{plugin} has no signed-in {account} account {label}"));
        };
        let before = self.active_label(plugin, account);
        self.set_active(plugin, account, &label);
        let lock = self.lock_of(plugin);
        let _held = lock.lock().await;
        if let Some(prev) = before.filter(|p| *p != label) {
            self.push_live(plugin, CLEAR, json!({"account": account, "label": prev})).await;
        }
        self.push_label(&r).await;
        self.account_changed(plugin, account, &label, "connected");
        Ok(json!({"ok": true, "account": account, "label": label, "active": true}))
    }

    fn set_account_state(&self, r: &AccountRow, state: &str) {
        let now = self.clock.now() as i64;
        let _ = self.sql(|db| db.execute("UPDATE v1_accounts SET state=?, updated_at=? WHERE plugin=? AND account=? AND label=?", params![state, now, r.plugin, r.account, r.label]));
        self.account_changed(&r.plugin, &r.account, &r.label, state);
    }

    fn account_changed(&self, plugin: &str, account: &str, label: &str, state: &str) {
        let _ = self.accounts.events.send(json!({"plugin": plugin, "account": account, "label": label, "state": state}));
        self.emit(plugin);
    }

    /// The shared Google client, when the user added one.
    fn google_client(&self) -> Option<Map<String, Value>> {
        let vals: Option<String> = self.sql(|db| db.query_row("SELECT vals FROM plugin_settings WHERE plugin=?", [GOOGLE_GROUP], |r| r.get::<_, Option<String>>(0)).optional()).ok().flatten().flatten();
        let v = super::parse(vals.as_deref().unwrap_or(""), json!({}));
        let id = v.get("GOOGLE_CLIENT_ID").and_then(Value::as_str).filter(|s| !s.trim().is_empty())?;
        let mut m = Map::new();
        m.insert("client_id".into(), json!(id.trim()));
        if let Some(s) = v.get("GOOGLE_CLIENT_SECRET").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
            m.insert("client_secret".into(), json!(s.trim()));
        }
        Some(m)
    }

    async fn tokens(&self, r: &str) -> Option<Value> {
        let (keys, r) = (self.keys.clone(), r.to_owned());
        tokio::task::spawn_blocking(move || keys.get_token(&r)).await.ok().and_then(Result::ok).flatten()
    }

    async fn put_tokens(&self, r: &str, t: &Value) -> Res<()> {
        let (keys, r, t) = (self.keys.clone(), r.to_owned(), t.clone());
        tokio::task::spawn_blocking(move || keys.put_token(&r, &t)).await.map_err(|e| V1Error::new("internal", e.to_string()))?.map_err(|e| V1Error::new("internal", e))
    }

    async fn drop_tokens(&self, r: &str) {
        let (keys, r) = (self.keys.clone(), r.to_owned());
        let _ = tokio::task::spawn_blocking(move || keys.remove_token(&r)).await;
    }

    /// The accounts section of the plugin's snapshot (no tokens): one row
    /// per signed-in label of each declared account, `{account, label,
    /// provider, title, subject, state, required}`; an account never signed
    /// in is a `default` row in state `none`; a signed-in row says whether
    /// it is the `active` label. A row whose provider's setup
    /// is missing (`setup_view`) adds `setup` (why sign-in can't start); one
    /// whose last sign-in failed adds `error`, and `fix: "setup"` when the
    /// provider refused its client.
    pub fn accounts_view(&self, id: &str) -> Value {
        let declared = self.declared_accounts(id);
        if declared.is_empty() {
            return json!([]);
        }
        let rows = self.account_rows(id);
        let mut out = Vec::new();
        for a in &declared {
            let missing = self.setup_missing(&a.provider);
            let row = |label: &str, subject: Option<&str>, state: &str| {
                let mut o = json!({"account": a.id, "label": label, "provider": a.provider, "title": a.label, "subject": subject, "state": state, "required": a.required});
                if missing {
                    o["setup"] = json!(self.stored_setup_error(&a.provider).unwrap_or_else(|| NEEDS_GOOGLE_CLIENT.to_owned()));
                }
                if let Some((why, fix)) = self.accounts.errors().get(&token_ref(id, &a.id, label)).filter(|_| state != "connected") {
                    o["error"] = json!(why);
                    if *fix {
                        o["fix"] = json!("setup");
                    }
                }
                o
            };
            let mine: Vec<&AccountRow> = rows.iter().filter(|r| r.account == a.id).collect();
            if mine.is_empty() {
                out.push(row(DEFAULT_LABEL, None, "none"));
            }
            let active = self.active_label(id, &a.id);
            for r in mine {
                let mut o = row(&r.label, Some(r.subject.as_str()).filter(|s| !s.is_empty()), &r.state);
                o["active"] = json!(active.as_deref() == Some(r.label.as_str()));
                out.push(o);
            }
        }
        json!(out)
    }

    /// A `plugin_settings` row's values (`{}` when none).
    fn group_vals(&self, group: &str) -> Map<String, Value> {
        let vals: Option<String> = self.sql(|db| db.query_row("SELECT vals FROM plugin_settings WHERE plugin=?", [group], |r| r.get::<_, Option<String>>(0)).optional()).ok().flatten().flatten();
        match super::parse(vals.as_deref().unwrap_or(""), json!({})) {
            Value::Object(m) => m,
            _ => Map::new(),
        }
    }

    /// Whether `provider`'s sign-in still lacks setup: a field of its
    /// setup without a value, or a stored one of the wrong shape.
    pub fn setup_missing(&self, provider: &str) -> bool {
        let Some((group, fields)) = provider_setup(provider) else { return false };
        let vals = self.group_vals(group);
        fields.iter().any(|f| !is_set(&vals, &js::string_of(&f["key"]))) || self.stored_setup_error(provider).is_some()
    }

    /// A stored value of `provider`'s setup that fails its shape (saved
    /// before the checks): what to tell the user, never the value.
    pub fn stored_setup_error(&self, provider: &str) -> Option<String> {
        let (group, fields) = provider_setup(provider)?;
        let vals = self.group_vals(group);
        fields.iter().find_map(|f| {
            let v = vals.get(&js::string_of(&f["key"])).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty())?;
            shape_error(f, v)?;
            let what = if f["secret"] == true { "is not a Client Secret" } else { "is not a Client ID" };
            let label = js::string_of(&f["label"]);
            Some(format!("The saved {} {what}. Change it in Setup.", if label.is_empty() { js::string_of(&f["key"]) } else { label }))
        })
    }

    /// The setup a plugin's declared accounts need, one entry per provider
    /// that has any: `{provider, ready, fields: [{key, label, placeholder,
    /// secret, hint, set, value?, pattern…}]}`, `ready` when every field is
    /// set in its shape (a stored one of the wrong shape: not ready, with
    /// `error`); `value` only for a set field that is not secret.
    pub fn setup_view(&self, id: &str) -> Value {
        let mut out: Vec<Value> = Vec::new();
        for a in self.declared_accounts(id) {
            if out.iter().any(|x| x["provider"] == json!(a.provider)) {
                continue;
            }
            let Some((group, fields)) = provider_setup(&a.provider) else { continue };
            let vals = self.group_vals(group);
            let fields: Vec<Value> = fields
                .into_iter()
                .map(|mut f| {
                    let key = js::string_of(&f["key"]);
                    f["set"] = json!(is_set(&vals, &key));
                    // a value that is no secret shows, so it can be changed
                    if f["secret"] != true && is_set(&vals, &key) {
                        f["value"] = vals[&key].clone();
                    }
                    f
                })
                .collect();
            let error = self.stored_setup_error(&a.provider);
            let ready = fields.iter().all(|f| f["set"] == true) && error.is_none();
            let mut entry = json!({"provider": a.provider, "ready": ready, "fields": fields});
            if let Some(e) = error {
                entry["error"] = json!(e);
            }
            out.push(entry);
        }
        json!(out)
    }

    /// `POST …/plugins/<id>/setup {values, clear?}`: the fields its setup
    /// declares, trimmed, saved in their provider's shared row (other keys
    /// refused, nothing saved then); an empty secret keeps the stored one,
    /// an empty other field clears it; `clear: true` removes every field of
    /// its setup. A value that changed expires the provider's signed-in
    /// accounts ([`Host::expire_provider`]). Every plugin sharing the
    /// provider hears the change. → `{ok, id, setup, accounts}` (no secret).
    pub async fn save_setup(&self, id: &str, values: Option<&Value>, clear: bool) -> Res {
        self.must_row(id)?;
        let mut providers: Vec<String> = Vec::new();
        for a in self.declared_accounts(id) {
            if provider_setup(&a.provider).is_some() && !providers.contains(&a.provider) {
                providers.push(a.provider);
            }
        }
        if providers.is_empty() {
            return V1Error::err("invalid_args", format!("{id} needs no setup"));
        }
        let values = match values {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(m)) => m.clone(),
            Some(_) => return V1Error::err("invalid_args", "values must map field keys to strings"),
        };
        // (group, key, value, secret) of each value sent, checked before any is saved
        let mut sent: Vec<(&'static str, String, String, bool)> = Vec::new();
        for (k, v) in &values {
            let found = providers.iter().filter_map(|p| provider_setup(p)).find_map(|(g, fs)| fs.into_iter().find(|f| f["key"] == json!(k)).map(|f| (g, f["secret"] == true)));
            let Some((group, secret)) = found else { return V1Error::err("invalid_args", format!("{id}'s setup has no field {}", js::slice_utf16(k, 80))) };
            let v = match v {
                Value::Null => String::new(),
                Value::String(s) => s.trim_matches(js::is_space).to_owned(),
                _ => return V1Error::err("invalid_args", format!("{} must be a string", js::slice_utf16(k, 80))),
            };
            sent.push((group, k.clone(), v, secret));
        }
        // the shapes, before anything is saved: swapped, then each value
        if !clear {
            for p in &providers {
                let Some((g, fields)) = provider_setup(p) else { continue };
                let mut after = self.group_vals(g);
                for (_, k, v, _) in sent.iter().filter(|x| x.0 == g && !x.2.is_empty()) {
                    after.insert(k.clone(), json!(v));
                }
                if let Some(why) = swapped(&fields, &after) {
                    return V1Error::err("invalid_args", why);
                }
                for (_, k, v, _) in sent.iter().filter(|x| x.0 == g && !x.2.is_empty()) {
                    if let Some(why) = fields.iter().find(|f| f["key"] == json!(k)).and_then(|f| shape_error(f, v)) {
                        return V1Error::err("invalid_args", why);
                    }
                }
            }
        }
        if clear {
            // every field, emptied (a secret too)
            sent = providers.iter().filter_map(|p| provider_setup(p)).flat_map(|(g, fs)| fs.into_iter().map(move |f| (g, js::string_of(&f["key"]), String::new(), false))).collect();
        }
        let mut groups: Vec<&'static str> = sent.iter().map(|x| x.0).collect();
        groups.sort_unstable();
        groups.dedup();
        let now = self.clock.now() as i64;
        let mut changed: Vec<&'static str> = Vec::new();
        for g in groups {
            let mut vals = self.group_vals(g);
            let before = vals.clone();
            for (_, k, v, secret) in sent.iter().filter(|x| x.0 == g) {
                if !v.is_empty() {
                    vals.insert(k.clone(), json!(v));
                } else if !secret {
                    vals.shift_remove(k);
                }
            }
            self.sql(|db| {
                db.execute_batch("CREATE TABLE IF NOT EXISTS plugin_settings(plugin TEXT PRIMARY KEY, vals TEXT, updated_at INTEGER)")?;
                if vals.is_empty() {
                    db.execute("DELETE FROM plugin_settings WHERE plugin=?", [g]).map(drop)
                } else {
                    db.execute("INSERT OR REPLACE INTO plugin_settings(plugin, vals, updated_at) VALUES(?,?,?)", params![g, js::stringify(&Value::Object(vals.clone())), now]).map(drop)
                }
            })?;
            if sent.iter().filter(|x| x.0 == g).any(|(_, k, ..)| before.get(k) != vals.get(k)) {
                changed.push(g);
            }
        }
        for p in providers.iter().filter(|p| provider_setup(p).is_some_and(|(g, _)| changed.contains(&g))) {
            self.expire_provider(p).await;
        }
        for r in self.rows() {
            if r.id == id || self.declared_accounts(&r.id).iter().any(|a| providers.contains(&a.provider)) {
                self.emit(&r.id);
            }
        }
        Ok(json!({"ok": true, "id": id, "setup": self.setup_view(id), "accounts": self.accounts_view(id)}))
    }

    /// The client of `provider` changed: every v1 plugin's signed-in
    /// account of it is `expired` and cleared in the plugin (its tokens were
    /// the old client's), and sign-ins begun with the old client end.
    pub async fn expire_provider(&self, provider: &str) {
        self.accounts.pending().retain(|_, p| p.provider != provider);
        for row in self.rows() {
            for r in self.account_rows(&row.id).into_iter().filter(|r| r.provider == provider && r.state == "connected") {
                self.clear_refresh(&r.token_ref);
                let now = self.clock.now() as i64;
                let _ = self.sql(|db| db.execute("UPDATE v1_accounts SET state='expired', updated_at=? WHERE plugin=? AND account=? AND label=?", params![now, r.plugin, r.account, r.label]));
                let lock = self.lock_of(&r.plugin);
                let _held = lock.lock().await;
                self.push_live(&r.plugin, CLEAR, json!({"account": r.account, "label": r.label})).await;
                self.account_changed(&r.plugin, &r.account, &r.label, "expired");
            }
        }
    }

    fn declared_one(&self, plugin: &str, account: &str) -> Res<Account> {
        let row = self.must_row(plugin)?;
        let declared = self.package_of(&row).map(|p| p.accounts).unwrap_or_default();
        declared.into_iter().find(|a| a.id == account).ok_or_else(|| V1Error::new("not_found", format!("{plugin} declares no account {}", js::slice_utf16(account, 40))))
    }

    /// `POST …/accounts/<account>/signin {label?, open?}`: the provider's
    /// authorize URL in the browser (`open: false`: answered instead); the
    /// callback finishes it.
    pub async fn account_signin(&self, plugin: &str, account: &str, b: &Value) -> Res {
        let decl = self.declared_one(plugin, account)?;
        let label = label_of(b)?;
        let open = b.get("open") != Some(&Value::Bool(false));
        let cfg = self.accounts.cfg();
        let flow = flow_id(plugin, account, &label);
        let redirect = oauth::fixed_redirect_url(cfg.port, &flow);
        let (verifier, challenge) = oauth::pkce();
        let state = random_state();
        let mut scopes = decl.scopes.clone();
        let (mut url, token_endpoint, auth_methods, client, extra) = if decl.provider == "google" {
            let Some(client) = self.google_client() else { return V1Error::err("needs_setup", NEEDS_GOOGLE_CLIENT) };
            // a client of the wrong shape (saved before the checks) never reaches Google
            if let Some(why) = self.stored_setup_error("google") {
                return V1Error::err("needs_setup", why);
            }
            for s in ["openid", "email"] {
                if !scopes.iter().any(|x| x == s) {
                    scopes.push(s.into());
                }
            }
            let url = Url::parse(&cfg.google_authorize).map_err(|e| V1Error::new("internal", e.to_string()))?;
            (url, cfg.google_token.clone(), vec!["client_secret_post".to_owned()], client, vec![("access_type", "offline"), ("prompt", "consent")])
        } else {
            let issuer = decl.provider.strip_prefix("oauth:").unwrap_or_default();
            let (url, endpoint, methods, client) = self.register_with(issuer, &redirect, &scopes).await?;
            (url, endpoint, methods, client, Vec::new())
        };
        let client_id = client.get("client_id").map(js::string_of).unwrap_or_default();
        oauth::set_param(&mut url, "response_type", "code");
        oauth::set_param(&mut url, "client_id", &client_id);
        oauth::set_param(&mut url, "redirect_uri", &redirect);
        if !scopes.is_empty() {
            oauth::set_param(&mut url, "scope", &scopes.join(" "));
        }
        oauth::set_param(&mut url, "state", &state);
        oauth::set_param(&mut url, "code_challenge", &challenge);
        oauth::set_param(&mut url, "code_challenge_method", "S256");
        for (k, v) in extra {
            oauth::set_param(&mut url, k, v);
        }
        let p = Pending {
            plugin: plugin.into(),
            account: account.into(),
            label: label.clone(),
            provider: decl.provider.clone(),
            state,
            verifier,
            redirect,
            token_endpoint,
            auth_methods,
            client,
            scopes,
            until: Instant::now() + PENDING_FOR,
        };
        self.accounts.errors().remove(&token_ref(plugin, account, &label));
        self.accounts.pending().insert(flow, p);
        let u = url.to_string();
        if open {
            (cfg.open)(&u);
        }
        self.emit(plugin);
        let mut out = json!({"ok": true, "redirected": true, "account": account, "label": label});
        if !open {
            out["url"] = json!(u);
        }
        Ok(out)
    }

    /// `oauth:<issuer>`: RFC 8414 metadata of the issuer itself, then RFC
    /// 7591 registration for this redirect. → (authorize URL, token
    /// endpoint, its auth methods, the client).
    async fn register_with(&self, issuer: &str, redirect: &str, scopes: &[String]) -> Res<(Url, String, Vec<String>, Map<String, Value>)> {
        let unavailable = |why: String| V1Error::new("plugin_unavailable", format!("{issuer}: {}", js::slice_utf16(&why, 300)));
        let as_url = Url::parse(issuer).map_err(|e| unavailable(e.to_string()))?;
        let f = Fetcher::default();
        let m = oauth::discover_as(&f, &as_url).await.map_err(unavailable)?.ok_or_else(|| unavailable("publishes no OAuth server metadata".into()))?;
        if !m.get("issuer").and_then(Value::as_str).is_some_and(|i| oauth::issuers_match(i, issuer)) {
            return Err(unavailable("the metadata names another issuer".into()));
        }
        if !m.get("response_types_supported").and_then(Value::as_array).is_some_and(|t| t.iter().any(|x| x == "code")) {
            return Err(unavailable("does not support response type code".into()));
        }
        if m.get("code_challenge_methods_supported").and_then(Value::as_array).is_some_and(|c| !c.iter().any(|x| x == "S256")) {
            return Err(unavailable("does not support PKCE S256".into()));
        }
        let mut meta = Map::new();
        meta.insert("client_name".into(), json!("Buwiz Bot"));
        meta.insert("redirect_uris".into(), json!([redirect]));
        meta.insert("grant_types".into(), json!(["authorization_code", "refresh_token"]));
        meta.insert("response_types".into(), json!(["code"]));
        meta.insert("token_endpoint_auth_method".into(), json!("none"));
        let scope = (!scopes.is_empty()).then(|| scopes.join(" "));
        let client = oauth::register(&f, &as_url, Some(&m), &meta, scope.as_deref()).await.map_err(|e| unavailable(e.message()))?;
        let authorize = m.get("authorization_endpoint").and_then(Value::as_str).and_then(|u| Url::parse(u).ok()).ok_or_else(|| unavailable("no authorization_endpoint".into()))?;
        let token = m.get("token_endpoint").and_then(Value::as_str).ok_or_else(|| unavailable("no token_endpoint".into()))?.to_owned();
        let methods = m.get("token_endpoint_auth_methods_supported").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
        Ok((authorize, token, methods, client))
    }

    /// Whether `/oauth/callback/<flow>` is an account sign-in of this host.
    pub fn account_pending(&self, flow: &str) -> bool {
        self.accounts.pending().contains_key(flow)
    }

    /// The callback: "refused" (400), "failed" (with why) or "connected".
    pub async fn account_callback(&self, flow: &str, code: Option<&str>, state: Option<&str>, error: Option<&str>) -> (&'static str, String) {
        let Some(p) = self.accounts.pending().get(flow).cloned() else { return ("refused", String::new()) };
        if code.is_none() && error.is_none() {
            return ("refused", String::new());
        }
        if code.is_some() && state != Some(p.state.as_str()) {
            return ("refused", String::new());
        }
        self.accounts.pending().remove(flow);
        if let Some(e) = error {
            self.accounts.errors().insert(token_ref(&p.plugin, &p.account, &p.label), (format!("Sign-in failed: {}", js::slice_utf16(e, 200)), false));
            self.emit(&p.plugin);
            return ("failed", js::slice_utf16(e, 200));
        }
        let params = vec![
            ("grant_type".into(), "authorization_code".into()),
            ("code".into(), code.unwrap_or_default().to_owned()),
            ("code_verifier".into(), p.verifier.clone()),
            ("redirect_uri".into(), p.redirect.clone()),
        ];
        let got = self.token_call(&p.token_endpoint, &p.auth_methods, &p.client, params).await;
        let r = token_ref(&p.plugin, &p.account, &p.label);
        let t = match got {
            Ok(t) => t,
            Err(e) => {
                let why = if e.bad_client && p.provider == "google" { GOOGLE_REFUSED_CLIENT.to_owned() } else { e.why };
                self.accounts.errors().insert(r, (why.clone(), e.bad_client && p.provider == "google"));
                self.emit(&p.plugin);
                return ("failed", why);
            }
        };
        self.accounts.errors().remove(&r);
        let r = AccountRow {
            plugin: p.plugin.clone(),
            account: p.account.clone(),
            label: p.label.clone(),
            provider: p.provider.clone(),
            subject: subject_of(t.get("id_token")),
            token_ref: token_ref(&p.plugin, &p.account, &p.label),
            state: "connected".into(),
            scopes: scopes_of(t.get("scope")).unwrap_or(p.scopes.clone()),
            updated_at: 0,
            active: true,
        };
        let mut blob = self.blob(&t, None);
        if p.provider != "google" {
            blob["token_endpoint"] = json!(p.token_endpoint);
            blob["auth_methods"] = json!(p.auth_methods);
            blob["client"] = Value::Object(p.client.clone());
        }
        if let Err(e) = self.put_tokens(&r.token_ref, &blob).await {
            self.emit(&p.plugin);
            return ("failed", e.message);
        }
        if let Err(e) = self.put_account(&r) {
            return ("failed", e.message);
        }
        // the new sign-in is the active one: the account's other labels leave the plugin
        let others: Vec<String> = self.account_rows(&p.plugin).into_iter().filter(|x| x.account == p.account && x.label != p.label && x.state == "connected").map(|x| x.label).collect();
        self.set_active(&p.plugin, &p.account, &p.label);
        let lock = self.lock_of(&p.plugin);
        let _held = lock.lock().await;
        for l in others {
            self.push_live(&p.plugin, CLEAR, json!({"account": p.account, "label": l})).await;
        }
        self.push_live(&p.plugin, SET, set_args(&r, &blob)).await;
        self.schedule_refresh(&r, &blob);
        self.account_changed(&r.plugin, &r.account, &r.label, "connected");
        ("connected", String::new())
    }

    /// A token request; the token response (`id_token` too).
    async fn token_call(&self, endpoint: &str, methods: &[String], client: &Map<String, Value>, params: Vec<(String, String)>) -> Result<Map<String, Value>, Refused> {
        let url = Url::parse(endpoint).map_err(|e| Refused { why: e.to_string(), bad_client: false })?;
        let mut md = Map::new();
        md.insert("token_endpoint".into(), json!(endpoint));
        md.insert("token_endpoint_auth_methods_supported".into(), json!(methods));
        oauth::token_request(&Fetcher::default(), &url, Some(&md), params, Some(client), None).await.map_err(|e| {
            let bad_client = matches!(&e, oauth::AuthError::OAuth { kind: oauth::Kind::InvalidClient | oauth::Kind::UnauthorizedClient, .. });
            Refused { why: js::slice_utf16(&e.message(), 300), bad_client }
        })
    }

    /// What the keystore keeps of a token response (`prev`'s refresh token
    /// when none came, and its client).
    fn blob(&self, t: &Map<String, Value>, prev: Option<&Value>) -> Value {
        let expires_in = t.get("expires_in").and_then(Value::as_f64).filter(|n| *n > 0.0).unwrap_or(3600.0);
        let expires_at = ((self.clock.now() / 1000.0).floor() + expires_in.floor()) as i64;
        let mut b = match prev {
            Some(Value::Object(p)) => p.clone(),
            _ => Map::new(),
        };
        b.insert("access_token".into(), t.get("access_token").cloned().unwrap_or(Value::Null));
        b.insert("token_type".into(), t.get("token_type").cloned().unwrap_or(json!("Bearer")));
        b.insert("expires_at".into(), json!(expires_at));
        if let Some(rt) = t.get("refresh_token").filter(|r| js::truthy(r)) {
            b.insert("refresh_token".into(), rt.clone());
        }
        Value::Object(b)
    }

    /// A refresh with the stored refresh token; the new blob, stored.
    async fn refresh_account(&self, r: &AccountRow) -> Result<Value, String> {
        let prev = self.tokens(&r.token_ref).await.ok_or("no stored tokens")?;
        let rt = prev.get("refresh_token").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or("no refresh token")?.to_owned();
        let (endpoint, methods, client) = if r.provider == "google" {
            (self.accounts.cfg().google_token, vec!["client_secret_post".to_owned()], self.google_client().ok_or(NEEDS_GOOGLE_CLIENT)?)
        } else {
            let endpoint = prev.get("token_endpoint").and_then(Value::as_str).ok_or("no token endpoint")?.to_owned();
            let methods = prev.get("auth_methods").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
            (endpoint, methods, prev.get("client").and_then(Value::as_object).cloned().unwrap_or_default())
        };
        let t = self.token_call(&endpoint, &methods, &client, vec![("grant_type".into(), "refresh_token".into()), ("refresh_token".into(), rt)]).await?;
        let blob = self.blob(&t, Some(&prev));
        self.put_tokens(&r.token_ref, &blob).await.map_err(|e| e.message)?;
        if let Some(s) = scopes_of(t.get("scope")) {
            let _ = self.put_account(&AccountRow { scopes: s, ..r.clone() });
        }
        Ok(blob)
    }

    /// A reserved account call; -32601 is `plugin_invalid` (the plugin
    /// declares accounts, so it must answer both).
    async fn push(&self, conn: &Conn, plugin: &str, name: &str, args: Value) -> Result<(), Fail> {
        match self.reserved(conn, name, args, self.timeouts.reserved).await {
            Ok(_) => Ok(()),
            Err(Fail::Rpc(e)) if e.code == mcp::METHOD_NOT_FOUND => refuse("plugin_invalid", format!("{plugin} does not implement {name}")),
            Err(Fail::V1(e)) => refuse("refused", e.message),
            Err(e) => Err(e),
        }
    }

    /// A push to the running plugin (none running: the next spawn pushes);
    /// -32601 fails it.
    async fn push_live(&self, plugin: &str, name: &str, args: Value) {
        let Ok(conn) = self.running_conn(plugin) else { return };
        match self.push(&conn, plugin, name, args).await {
            Ok(()) => {}
            Err(Fail::V1(e)) if e.code == "plugin_invalid" => {
                self.stop_conn(plugin).await;
                self.set_state(plugin, "failed", Some("plugin_invalid"), &e.message);
            }
            Err(e) => eprintln!("  v1 {plugin}: {name}: {}", js::slice_utf16(e.message(), 200)),
        }
    }

    /// After provision (the gate's connection): `buwiz.account.set` for
    /// each account's active label, refreshed first when it is within 5
    /// minutes of expiry (a failed refresh: expired, nothing pushed); the
    /// other connected labels are only kept fresh.
    pub(super) async fn push_accounts(&self, id: &str, conn: &Conn, pkg: &Package) -> Result<(), V1Error> {
        for r in self.account_rows(id).into_iter().filter(|r| r.state == "connected" && pkg.accounts.iter().any(|a| a.id == r.account)) {
            if self.active_label(id, &r.account).as_deref() != Some(r.label.as_str()) {
                if let Some(t) = self.tokens(&r.token_ref).await {
                    self.schedule_refresh(&r, &t);
                }
                continue;
            }
            let Some(mut t) = self.tokens(&r.token_ref).await else {
                self.set_account_state(&r, "expired");
                continue;
            };
            if self.due(&t) {
                match self.refresh_account(&r).await {
                    Ok(fresh) => t = fresh,
                    Err(why) => {
                        eprintln!("  v1 {id}: account {}/{}: refresh failed: {}", r.account, r.label, js::slice_utf16(&why, 200));
                        self.set_account_state(&r, "expired");
                        continue;
                    }
                }
            }
            match self.push(conn, id, SET, set_args(&r, &t)).await {
                Ok(()) => self.schedule_refresh(&r, &t),
                Err(Fail::V1(e)) if e.code == "plugin_invalid" => return Err(e),
                Err(e) => eprintln!("  v1 {id}: {SET}: {}", js::slice_utf16(e.message(), 200)),
            }
        }
        Ok(())
    }

    /// Within 5 minutes of `expires_at`.
    fn due(&self, t: &Value) -> bool {
        t.get("expires_at").and_then(Value::as_f64).is_none_or(|e| e * 1000.0 - REFRESH_EARLY_MS <= self.clock.now())
    }

    fn clear_refresh(&self, r: &str) {
        if let Some(t) = self.accounts.timers.lock().unwrap_or_else(|e| e.into_inner()).remove(r) {
            self.clock.clear(t);
        }
    }

    /// The refresh 5 minutes before `expires_at`.
    fn schedule_refresh(&self, r: &AccountRow, t: &Value) {
        let Some(expires) = t.get("expires_at").and_then(Value::as_f64) else { return };
        self.clear_refresh(&r.token_ref);
        let delay = (expires * 1000.0 - REFRESH_EARLY_MS - self.clock.now()).max(0.0) as u64;
        let (me, row) = (self.me.clone(), r.clone());
        let timer = self.clock.set_timeout(
            delay,
            Box::new(move || {
                let Some(host) = me.upgrade() else { return };
                tokio::spawn(async move { host.refresh_tick(&row).await });
            }),
        );
        self.accounts.timers.lock().unwrap_or_else(|e| e.into_inner()).insert(r.token_ref.clone(), timer);
    }

    /// A refresh timer fired: refresh and push again while the plugin
    /// runs; a failure is `expired` and `buwiz.account.clear`.
    async fn refresh_tick(&self, r: &AccountRow) {
        self.accounts.timers.lock().unwrap_or_else(|e| e.into_inner()).remove(&r.token_ref);
        if self.with_rt(&r.plugin, |rt| rt.state) != "running" {
            return;
        }
        let lock = self.lock_of(&r.plugin);
        let _held = lock.lock().await;
        let Some(r) = self.account_row(&r.plugin, &r.account, &r.label).filter(|x| x.state == "connected") else { return };
        // every connected label stays fresh; only the active one is pushed
        let active = self.active_label(&r.plugin, &r.account).as_deref() == Some(r.label.as_str());
        match self.refresh_account(&r).await {
            Ok(t) => {
                if active {
                    self.push_live(&r.plugin, SET, set_args(&r, &t)).await;
                }
                self.schedule_refresh(&r, &t);
                self.account_changed(&r.plugin, &r.account, &r.label, "connected");
            }
            Err(why) => {
                eprintln!("  v1 {}: account {}/{}: refresh failed: {}", r.plugin, r.account, r.label, js::slice_utf16(&why, 200));
                let _ = self.sql(|db| db.execute("UPDATE v1_accounts SET state='expired' WHERE plugin=? AND account=? AND label=?", [&r.plugin, &r.account, &r.label]));
                if active {
                    self.push_live(&r.plugin, CLEAR, json!({"account": r.account, "label": r.label})).await;
                }
                self.account_changed(&r.plugin, &r.account, &r.label, "expired");
            }
        }
    }

    /// `POST …/accounts/<account>/signout {label}`: tokens gone, the row
    /// `none`, `buwiz.account.clear` pushed.
    pub async fn account_signout(&self, plugin: &str, account: &str, b: &Value) -> Res {
        self.end_account(plugin, account, b, false).await
    }

    /// `POST …/accounts/<account>/remove {label}`: as sign-out, and the row
    /// goes.
    pub async fn account_remove(&self, plugin: &str, account: &str, b: &Value) -> Res {
        self.end_account(plugin, account, b, true).await
    }

    async fn end_account(&self, plugin: &str, account: &str, b: &Value, remove: bool) -> Res {
        self.declared_one(plugin, account)?;
        let label = label_of(b)?;
        let was_active = self.active_label(plugin, account).as_deref() == Some(label.as_str());
        let flow = flow_id(plugin, account, &label);
        let was_pending = self.accounts.pending().remove(&flow).is_some();
        let row = self.account_row(plugin, account, &label);
        if row.is_none() && !was_pending {
            return V1Error::err("not_found", format!("{plugin} has no {account} account {label}"));
        }
        let r = token_ref(plugin, account, &label);
        self.accounts.errors().remove(&r);
        self.clear_refresh(&r);
        self.drop_tokens(&r).await;
        if remove {
            self.sql(|db| db.execute("DELETE FROM v1_accounts WHERE plugin=? AND account=? AND label=?", [plugin, account, &label]))?;
        } else if row.is_some() {
            let now = self.clock.now() as i64;
            self.sql(|db| db.execute("UPDATE v1_accounts SET state='none', updated_at=? WHERE plugin=? AND account=? AND label=?", params![now, plugin, account, label]))?;
        }
        let _ = self.sql(|db| db.execute("UPDATE v1_accounts SET active=0 WHERE plugin=? AND account=? AND label=?", [plugin, account, &label]));
        let mut next = None;
        if row.is_some() {
            let lock = self.lock_of(plugin);
            let _held = lock.lock().await;
            self.push_live(plugin, CLEAR, json!({"account": account, "label": label})).await;
            // the active one signed out: another connected label is
            if was_active {
                if let Some(n) = self.active_label(plugin, account) {
                    self.set_active(plugin, account, &n);
                    if let Some(r) = self.account_row(plugin, account, &n) {
                        self.push_label(&r).await;
                    }
                    next = Some(n);
                }
            }
        }
        let state = if remove { "removed" } else { "none" };
        self.account_changed(plugin, account, &label, state);
        if let Some(n) = &next {
            self.account_changed(plugin, account, n, "connected");
        }
        Ok(json!({"ok": true, "account": account, "label": label, "state": state, "active": next}))
    }

    /// Uninstall: every account's tokens, rows, timers and sign-ins.
    pub(super) async fn forget_accounts(&self, plugin: &str) {
        for r in self.account_rows(plugin) {
            self.clear_refresh(&r.token_ref);
            self.drop_tokens(&r.token_ref).await;
        }
        self.accounts.pending().retain(|_, p| p.plugin != plugin);
        self.accounts.errors().retain(|r, _| !r.starts_with(&format!("{plugin}/")));
        let _ = self.sql(|db| db.execute("DELETE FROM v1_accounts WHERE plugin=?", [plugin]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_subject_is_the_id_tokens_email_else_its_sub() {
        let jwt = |claims: Value| format!("e30.{}.sig", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string()));
        assert_eq!(subject_of(Some(&json!(jwt(json!({"email": "u@example.com", "sub": "1"}))))), "u@example.com");
        assert_eq!(subject_of(Some(&json!(jwt(json!({"sub": "1"}))))), "1");
        assert_eq!(subject_of(Some(&json!("not a jwt"))), "");
        assert_eq!(subject_of(None), "");
    }

    #[test]
    fn labels_default_and_keep_to_their_grammar() {
        assert_eq!(label_of(&json!({})).unwrap(), "default");
        assert_eq!(label_of(&json!({"label": ""})).unwrap(), "default");
        assert_eq!(label_of(&json!({"label": "work@example.com"})).unwrap(), "work@example.com");
        assert_eq!(label_of(&json!({"label": "a b"})).unwrap_err().code, "invalid_args");
        assert_eq!(label_of(&json!({"label": 5})).unwrap_err().code, "invalid_args");
        assert_eq!(flow_id("app.buwiz.x", "google", "default"), "account:app.buwiz.x/google/default");
    }

    #[test]
    fn the_google_fields_check_the_client_shapes_and_a_swap() {
        let [id, secret] = google_fields();
        let cid = concat!("123-abc.apps", ".googleusercontent.com");
        assert_eq!(shape_error(&id, cid), None);
        for bad in ["GOCSPX-abc", concat!("123-abc.apps", ".googleusercontent.com.evil"), "abc.apps.googleusercontent.com", "123-ABC.apps.googleusercontent.com", ""] {
            assert_eq!(shape_error(&id, bad).as_deref(), Some("A Client ID ends with .apps.googleusercontent.com"), "{bad}");
        }
        for ok in ["GOCSPX-abc_123", "an-older-secret"] {
            assert_eq!(shape_error(&secret, ok), None, "{ok}");
        }
        assert_eq!(shape_error(&secret, cid).as_deref(), Some("This looks like a Client ID: the secret usually starts with GOCSPX-"));
        let fields = [id, secret];
        let vals = |i: &str, s: &str| json!({"GOOGLE_CLIENT_ID": i, "GOOGLE_CLIENT_SECRET": s}).as_object().unwrap().clone();
        let swap = swapped(&fields, &vals("GOCSPX-abc", cid)).unwrap();
        assert_eq!(swap, "The Client ID and Secret look swapped: the Client ID ends with .apps.googleusercontent.com, and the secret usually starts with GOCSPX-.");
        assert!(!swap.contains("GOCSPX-abc") && !swap.contains(cid), "no value in the message");
        assert_eq!(swapped(&fields, &vals(cid, "GOCSPX-abc")), None);
        assert_eq!(swapped(&fields, &vals(cid, cid)), None, "both IDs: the secret's own check says so");
        assert_eq!(swapped(&fields, &vals("GOCSPX-abc", "")), None);
    }

    #[test]
    fn an_accounts_table_from_before_active_gains_the_column() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE v1_accounts(plugin TEXT NOT NULL, account TEXT NOT NULL, label TEXT NOT NULL, provider TEXT NOT NULL, subject TEXT NOT NULL DEFAULT '', token_ref TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'none', scopes TEXT NOT NULL DEFAULT '[]', updated_at INTEGER NOT NULL, PRIMARY KEY(plugin, account, label)); INSERT INTO v1_accounts(plugin,account,label,provider,token_ref,state,updated_at) VALUES('p','google','default','google','r','connected',1)").unwrap();
        migrate(&db).unwrap();
        migrate(&db).unwrap();
        let r = db.query_row("SELECT * FROM v1_accounts", [], AccountRow::from_sql).unwrap();
        assert_eq!((r.label.as_str(), r.state.as_str(), r.active), ("default", "connected", false));
    }

    #[test]
    fn set_args_carry_the_token_and_never_the_refresh_token() {
        let r = AccountRow {
            plugin: "p".into(),
            account: "google".into(),
            label: "default".into(),
            provider: "google".into(),
            subject: "u@x".into(),
            token_ref: "r".into(),
            state: "connected".into(),
            scopes: vec!["s".into()],
            updated_at: 0,
            active: true,
        };
        let a = set_args(&r, &json!({"access_token": "at", "refresh_token": "rt", "expires_at": 9}));
        assert_eq!(a, json!({"account": "google", "label": "default", "provider": "google", "subject": "u@x", "scopes": ["s"], "access_token": "at", "token_type": "Bearer", "expires_at": 9}));
    }
}
