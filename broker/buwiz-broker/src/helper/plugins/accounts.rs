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
    scopes TEXT NOT NULL DEFAULT '[]', updated_at INTEGER NOT NULL,
    PRIMARY KEY(plugin, account, label));
";

pub const GOOGLE_AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const GOOGLE_TOKEN: &str = "https://oauth2.googleapis.com/token";
/// `plugin_settings`' shared Google client (`hub::market::catalog::GOOGLE_GROUP`).
const GOOGLE_GROUP: &str = "@google";
pub const DEFAULT_LABEL: &str = "default";
pub const NEEDS_GOOGLE_CLIENT: &str = "Add your Google client in Setup";
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
    events: broadcast::Sender<Value>,
}

impl Accounts {
    pub(super) fn new() -> Self {
        Self { cfg: Mutex::new(AccountsConfig::default()), pending: Mutex::new(HashMap::new()), timers: Mutex::new(HashMap::new()), events: broadcast::channel(256).0 }
    }

    fn cfg(&self) -> AccountsConfig {
        self.cfg.lock().unwrap_or_else(|e| e.into_inner()).clone()
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
                "INSERT OR REPLACE INTO v1_accounts(plugin,account,label,provider,subject,token_ref,state,scopes,updated_at) VALUES(?,?,?,?,?,?,?,?,?)",
                params![r.plugin, r.account, r.label, r.provider, r.subject, r.token_ref, r.state, js::stringify(&json!(r.scopes)), now],
            )
        })?;
        Ok(())
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
    /// in is a `default` row in state `none`. A Google row the shared
    /// client is missing for adds `setup` (why sign-in can't start).
    pub fn accounts_view(&self, id: &str) -> Value {
        let declared = self.declared_accounts(id);
        if declared.is_empty() {
            return json!([]);
        }
        let rows = self.account_rows(id);
        let google = declared.iter().any(|a| a.provider == "google") && self.google_client().is_some();
        let mut out = Vec::new();
        for a in &declared {
            let row = |label: &str, subject: Option<&str>, state: &str| {
                let mut o = json!({"account": a.id, "label": label, "provider": a.provider, "title": a.label, "subject": subject, "state": state, "required": a.required});
                if a.provider == "google" && !google {
                    o["setup"] = json!(NEEDS_GOOGLE_CLIENT);
                }
                o
            };
            let mine: Vec<&AccountRow> = rows.iter().filter(|r| r.account == a.id).collect();
            if mine.is_empty() {
                out.push(row(DEFAULT_LABEL, None, "none"));
            }
            for r in mine {
                out.push(row(&r.label, Some(r.subject.as_str()).filter(|s| !s.is_empty()), &r.state));
            }
        }
        json!(out)
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
        let t = match got {
            Ok(t) => t,
            Err(why) => {
                self.emit(&p.plugin);
                return ("failed", why);
            }
        };
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
        let lock = self.lock_of(&p.plugin);
        let _held = lock.lock().await;
        self.push_live(&p.plugin, SET, set_args(&r, &blob)).await;
        self.schedule_refresh(&r, &blob);
        self.account_changed(&r.plugin, &r.account, &r.label, "connected");
        ("connected", String::new())
    }

    /// A token request; the token response (`id_token` too).
    async fn token_call(&self, endpoint: &str, methods: &[String], client: &Map<String, Value>, params: Vec<(String, String)>) -> Result<Map<String, Value>, String> {
        let url = Url::parse(endpoint).map_err(|e| e.to_string())?;
        let mut md = Map::new();
        md.insert("token_endpoint".into(), json!(endpoint));
        md.insert("token_endpoint_auth_methods_supported".into(), json!(methods));
        oauth::token_request(&Fetcher::default(), &url, Some(&md), params, Some(client), None).await.map_err(|e| js::slice_utf16(&e.message(), 300))
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
    /// each connected account, refreshed first when it is within 5 minutes
    /// of expiry (a failed refresh: expired, nothing pushed).
    pub(super) async fn push_accounts(&self, id: &str, conn: &Conn, pkg: &Package) -> Result<(), V1Error> {
        for r in self.account_rows(id).into_iter().filter(|r| r.state == "connected" && pkg.accounts.iter().any(|a| a.id == r.account)) {
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
        match self.refresh_account(&r).await {
            Ok(t) => {
                self.push_live(&r.plugin, SET, set_args(&r, &t)).await;
                self.schedule_refresh(&r, &t);
                self.account_changed(&r.plugin, &r.account, &r.label, "connected");
            }
            Err(why) => {
                eprintln!("  v1 {}: account {}/{}: refresh failed: {}", r.plugin, r.account, r.label, js::slice_utf16(&why, 200));
                let _ = self.sql(|db| db.execute("UPDATE v1_accounts SET state='expired' WHERE plugin=? AND account=? AND label=?", [&r.plugin, &r.account, &r.label]));
                self.push_live(&r.plugin, CLEAR, json!({"account": r.account, "label": r.label})).await;
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
        let flow = flow_id(plugin, account, &label);
        let was_pending = self.accounts.pending().remove(&flow).is_some();
        let row = self.account_row(plugin, account, &label);
        if row.is_none() && !was_pending {
            return V1Error::err("not_found", format!("{plugin} has no {account} account {label}"));
        }
        let r = token_ref(plugin, account, &label);
        self.clear_refresh(&r);
        self.drop_tokens(&r).await;
        if remove {
            self.sql(|db| db.execute("DELETE FROM v1_accounts WHERE plugin=? AND account=? AND label=?", [plugin, account, &label]))?;
        } else if row.is_some() {
            let now = self.clock.now() as i64;
            self.sql(|db| db.execute("UPDATE v1_accounts SET state='none', updated_at=? WHERE plugin=? AND account=? AND label=?", params![now, plugin, account, label]))?;
        }
        if row.is_some() {
            let lock = self.lock_of(plugin);
            let _held = lock.lock().await;
            self.push_live(plugin, CLEAR, json!({"account": account, "label": label})).await;
        }
        let state = if remove { "removed" } else { "none" };
        self.account_changed(plugin, account, &label, state);
        Ok(json!({"ok": true, "account": account, "label": label, "state": state}))
    }

    /// Uninstall: every account's tokens, rows, timers and sign-ins.
    pub(super) async fn forget_accounts(&self, plugin: &str) {
        for r in self.account_rows(plugin) {
            self.clear_refresh(&r.token_ref);
            self.drop_tokens(&r.token_ref).await;
        }
        self.accounts.pending().retain(|_, p| p.plugin != plugin);
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
        };
        let a = set_args(&r, &json!({"access_token": "at", "refresh_token": "rt", "expires_at": 9}));
        assert_eq!(a, json!({"account": "google", "label": "default", "provider": "google", "subject": "u@x", "scopes": ["s"], "access_token": "at", "token_type": "Bearer", "expires_at": 9}));
    }
}
