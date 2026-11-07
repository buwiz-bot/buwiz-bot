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
//! A first-party plugin also learns the whole list: every push to it carries
//! `accounts`, each signed-in label as `{account, label, active}`
//! ([`Host::signed_in`]). Never a token or a subject, and a label that is an
//! address stays out of it, because a plugin's titles reach the model: if that
//! label is the active one its row stays, with no label. Its
//! tool asks for a switch with `_meta["buwiz/account_use"]`, on a person's
//! call only (`helper::sessions`); the Bot then calls `use`.
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
use crate::hub::routines::plugins::first_party;
pub const DEFAULT_LABEL: &str = "default";
pub const NEEDS_GOOGLE_CLIENT: &str = "Add your Google client in Setup";
pub const GOOGLE_REFUSED_CLIENT: &str = "Google refused this client. Check the Client ID and Secret.";
const REFRESH_EARLY_MS: f64 = 5.0 * 60.0 * 1000.0;
/// How long a refresh that got no answer waits to try again.
const REFRESH_RETRY_MS: u64 = 60 * 1000;
/// The longest a refresh answered by some other server waits between tries.
const REFRESH_BACKOFF_MAX_MS: u64 = 30 * 60 * 1000;
/// How long a sign-in goes unrefreshed before its row says so.
const REFRESH_STUCK_MS: f64 = 6.0 * 60.0 * 60.0 * 1000.0;
/// How long a provider has to answer a token request.
const TOKEN_WAIT: Duration = Duration::from_secs(20);
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
    /// How long a provider has to answer a token request.
    pub token_wait: Duration,
}

impl Default for AccountsConfig {
    #[cfg(not(feature = "test-google"))]
    fn default() -> Self {
        Self { port: 8787, google_authorize: GOOGLE_AUTHORIZE.into(), google_token: GOOGLE_TOKEN.into(), token_wait: TOKEN_WAIT, open: Arc::new(oauth::open_url) }
    }

    /// A test build's sign-in goes nowhere until [`AccountsConfig::from_env`]'s
    /// answer is set: no path to the real Google, no browser.
    #[cfg(feature = "test-google")]
    fn default() -> Self {
        const NOWHERE: &str = "http://127.0.0.1:0/";
        Self { port: 8787, google_authorize: NOWHERE.into(), google_token: NOWHERE.into(), token_wait: TOKEN_WAIT, open: Arc::new(|_| {}) }
    }
}

impl AccountsConfig {
    /// A release: Google, and the system browser. Nothing can change that,
    /// so a test Bot started on this build stops here rather than send its
    /// sign-in to the real Google.
    #[cfg(not(feature = "test-google"))]
    pub fn from_env(port: u16) -> Result<Self, String> {
        Self::for_release(port, std::env::var_os(TEST_GOOGLE_ENV).as_deref())
    }

    #[cfg(not(feature = "test-google"))]
    fn for_release(port: u16, value: Option<&std::ffi::OsStr>) -> Result<Self, String> {
        match value {
            Some(_) => Err(format!("{TEST_GOOGLE_ENV} is set, but this broker isn't a test-google build; not starting, so a test Bot can't reach the real Google")),
            None => Ok(Self { port, ..Self::default() }),
        }
    }

    /// A test build signs in at the stub or not at all: it never reaches the
    /// real Google, which would open the owner's browser.
    #[cfg(feature = "test-google")]
    pub fn from_env(port: u16) -> Result<Self, String> {
        Self::for_test(port, std::env::var_os(TEST_GOOGLE_ENV).as_deref())
    }

    /// [`Self::from_env`] with the variable's value in hand. A value that isn't
    /// text, or isn't the loopback stub, is refused like a missing one.
    #[cfg(feature = "test-google")]
    fn for_test(port: u16, value: Option<&std::ffi::OsStr>) -> Result<Self, String> {
        value.and_then(std::ffi::OsStr::to_str).and_then(|base| stub_google(port, base)).ok_or_else(|| format!("{TEST_GOOGLE_ENV} must be http://127.0.0.1:<port>; not starting, so a test Bot can't reach the real Google"))
    }
}

/// Names the stub Google on loopback, in a test build. A release refuses it.
pub const TEST_GOOGLE_ENV: &str = "BUWIZ_TEST_GOOGLE";

/// `http://127.0.0.1:<port>` and the rest, split: `None` for anything else.
#[cfg(feature = "test-google")]
fn loopback(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("http://")?;
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let port = host.strip_prefix("127.0.0.1:")?;
    (!port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())).then_some((host, path))
}

/// Sign-in against the stub at `base`, which must be on loopback: consent is
/// a GET of its authorize page, so no browser opens on the owner's desktop.
#[cfg(feature = "test-google")]
fn stub_google(port: u16, base: &str) -> Option<AccountsConfig> {
    let base = base.trim_end_matches('/');
    loopback(base).filter(|(_, path)| path.is_empty()).map(|_| AccountsConfig {
        port,
        token_wait: TOKEN_WAIT,
        google_authorize: format!("{base}/o/oauth2/v2/auth"),
        google_token: format!("{base}/token"),
        open: Arc::new(|url: &str| {
            let url = url.to_owned();
            std::thread::spawn(move || {
                if let Err(e) = follow(&url) {
                    eprintln!("buwiz-broker: stub sign-in: {e}");
                }
            });
        }),
    })
}

/// GETs a loopback `url` and the redirects it answers, as the browser would.
/// Every hop must stay on loopback, and each read waits ten seconds at most.
#[cfg(feature = "test-google")]
fn follow(url: &str) -> Result<(), String> {
    use std::io::{Read as _, Write as _};
    const WAIT: Duration = Duration::from_secs(10);
    let mut url = url.to_owned();
    for _ in 0..4 {
        let (host, path) = loopback(&url).ok_or("a hop off loopback was refused")?;
        let path = if path.is_empty() { "/" } else { path };
        let mut stream = std::net::TcpStream::connect(host).map_err(|e| e.to_string())?;
        stream.set_read_timeout(Some(WAIT)).map_err(|e| e.to_string())?;
        stream.write_all(format!("GET {path} HTTP/1.1\r\nhost: {host}\r\nconnection: close\r\n\r\n").as_bytes()).map_err(|e| e.to_string())?;
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).map_err(|e| e.to_string())?;
        let answer = String::from_utf8_lossy(&answer);
        let head = answer.split("\r\n\r\n").next().unwrap_or_default();
        let next = head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("location")).map(|(_, v)| v.trim().to_owned()));
        match next {
            Some(location) => url = location,
            None if head.split(' ').nth(1).is_some_and(|code| code.starts_with('2')) => return Ok(()),
            None => return Err(head.lines().next().unwrap_or("no answer").to_owned()),
        }
    }
    Err("too many redirects".to_owned())
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
    /// Each token ref a refresh is still trying: since when, and how many tries.
    retrying: Mutex<HashMap<String, (f64, u32)>>,
    /// The last failed sign-in of each token ref: (why, the setup fixes it).
    errors: Mutex<HashMap<String, (String, bool)>>,
    events: broadcast::Sender<Value>,
}

impl Accounts {
    pub(super) fn new() -> Self {
        Self { cfg: Mutex::new(AccountsConfig::default()), pending: Mutex::new(HashMap::new()), timers: Mutex::new(HashMap::new()), retrying: Mutex::new(HashMap::new()), errors: Mutex::new(HashMap::new()), events: broadcast::channel(256).0 }
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

/// Whether a sign-in lacks a scope its package now declares. A sign-in with no
/// scopes on record says nothing either way, so it isn't flagged.
fn lacks_scopes(declared: &[String], granted: &[String]) -> bool {
    !granted.is_empty() && declared.iter().any(|d| !granted.contains(d))
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

/// A field's shape checks and a swap (`buwiz_protocol::setup`, the app's
/// too).
pub use buwiz_protocol::setup::{shape_error, swapped};

fn is_set(vals: &Map<String, Value>, key: &str) -> bool {
    vals.get(key).and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty())
}

/// A failed token request: why, whether the provider refused the client
/// itself (a wrong Client ID or Secret), and what to do about a refresh.
struct Refused {
    why: String,
    bad_client: bool,
    after: After,
}

impl From<Refused> for String {
    fn from(r: Refused) -> String {
        r.why
    }
}

/// What a failed refresh leaves to do. Only a refusal ends a sign-in.
#[derive(Clone, Copy, PartialEq)]
enum After {
    Expire,
    Retry,
    /// Some other server answered, which may last: try less and less often.
    BackOff,
}

impl From<oauth::Verdict> for After {
    fn from(v: oauth::Verdict) -> Self {
        match v {
            oauth::Verdict::Refused => After::Expire,
            oauth::Verdict::Unanswered => After::Retry,
            oauth::Verdict::Stray => After::BackOff,
        }
    }
}

/// A refresh that failed: why, and what's left to do.
struct RefreshFailed {
    why: String,
    after: After,
}

impl From<&str> for RefreshFailed {
    fn from(why: &str) -> Self {
        Self { why: why.to_owned(), after: After::Expire }
    }
}

impl From<Refused> for RefreshFailed {
    fn from(r: Refused) -> Self {
        Self { why: r.why, after: r.after }
    }
}

/// How long try number `tries` waits after it fails.
fn retry_delay(after: After, tries: u32) -> u64 {
    match after {
        After::BackOff => (REFRESH_RETRY_MS << (tries - 1).min(5)).min(REFRESH_BACKOFF_MAX_MS),
        _ => REFRESH_RETRY_MS,
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

    /// Whether `row` is an approved plugin of the first-party publisher.
    pub fn first_party(&self, row: &Row) -> bool {
        first_party(row, &self.trust())
    }

    /// Whether `plugin` is one told the list, so a label leaving it has news
    /// for the plugin even if no token of that label was out.
    fn told_the_list(&self, plugin: &str) -> bool {
        self.row(plugin).is_some_and(|r| self.first_party(&r))
    }

    /// `plugin`'s signed-in labels, `[{account, label, active}]`: what a push
    /// tells a first-party plugin. A label may be typed as an address, and
    /// this list is for titles, which reach the model, so that one is left
    /// out: the Bot's own account page still switches to it, and no plugin
    /// can ask to. When it's the active one its row stays with no label, so
    /// the list never says nobody is active while a token is out.
    fn signed_in(&self, plugin: &str) -> Value {
        let rows = self.account_rows(plugin).into_iter().filter(|r| r.state == "connected");
        let view = |r: AccountRow| {
            let active = self.active_label(plugin, &r.account).as_deref() == Some(r.label.as_str());
            match (r.label.contains('@'), active) {
                (false, _) => Some(json!({"account": r.account, "label": r.label, "active": active})),
                (true, true) => Some(json!({"account": r.account, "active": true})),
                (true, false) => None,
            }
        };
        json!(rows.filter_map(view).collect::<Vec<_>>())
    }

    /// Marks `label` the active one of `plugin`'s `account` (the others not).
    fn set_active(&self, plugin: &str, account: &str, label: &str) {
        let _ = self.sql(|db| db.execute("UPDATE v1_accounts SET active=(label=?) WHERE plugin=? AND account=?", [label, plugin, account]));
    }

    /// Pushes `r`'s tokens to the running plugin (refreshed first when due)
    /// and keeps them fresh; `false` when it had none or the refresh was
    /// refused (it is `expired` then). A refresh nobody answered pushes what
    /// it holds and tries again. The caller holds the plugin's lock.
    async fn push_label(&self, r: &AccountRow) -> bool {
        let Some(mut t) = self.tokens(&r.token_ref).await else {
            self.expire_label(r).await;
            return false;
        };
        if self.due(&t) {
            match self.refresh_account(r).await {
                Ok(fresh) => t = fresh,
                Err(e) if e.after != After::Expire => {
                    self.push_live(&r.plugin, SET, set_args(r, &t)).await;
                    self.schedule_retry(r, e.after);
                    return true;
                }
                Err(_) => {
                    self.expire_label(r).await;
                    return false;
                }
            }
        }
        if self.push_live(&r.plugin, SET, set_args(r, &t)).await {
            self.schedule_refresh(r, &t);
        } else {
            self.schedule_retry(r, After::Retry);
        }
        true
    }

    /// `r` can't be pushed: it is `expired`, and a first-party plugin's list
    /// loses it, though no token of it was out. Any other plugin never had
    /// this label, so it hears nothing. The caller holds the lock.
    async fn expire_label(&self, r: &AccountRow) {
        self.set_account_state(r, "expired");
        if self.told_the_list(&r.plugin) {
            self.push_live(&r.plugin, CLEAR, json!({"account": r.account, "label": r.label})).await;
        }
    }

    /// The active label of `plugin`'s `account` is gone (signed out,
    /// expired): the next connected one becomes active and is pushed, each
    /// that cannot be pushed expired in turn; → the one pushed. The caller
    /// holds the plugin's lock.
    async fn promote_next(&self, plugin: &str, account: &str) -> Option<String> {
        while let Some(n) = self.active_label(plugin, account) {
            self.set_active(plugin, account, &n);
            let Some(r) = self.account_row(plugin, account, &n) else { return None };
            if self.push_label(&r).await {
                self.account_changed(plugin, account, &n, "connected");
                return Some(n);
            }
        }
        None
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
        if !self.push_label(&r).await {
            // its tokens could not be pushed (expired now): another one is
            let next = self.promote_next(plugin, account).await;
            return Err(V1Error::new("needs_signin", format!("{label} needs a sign-in again{}", next.map(|n| format!("; {n} is active")).unwrap_or_default())));
        }
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
    /// it is the `active` label; a row whose sign-in waits on its callback
    /// is `pending` (a new label's has a row of its own). A row whose provider's setup
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
                // a sign-in waiting on its callback (gone after a restart or once finished)
                if self.accounts.pending().contains_key(&flow_id(id, &a.id, label)) {
                    o["pending"] = json!(true);
                }
                // a failed sign-in's reason, a connected label's too (its re-sign-in failed)
                if let Some((why, fix)) = self.accounts.errors().get(&token_ref(id, &a.id, label)) {
                    o["error"] = json!(why);
                    if *fix {
                        o["fix"] = json!("setup");
                    }
                }
                if state == "connected" && self.refresh_stuck(&token_ref(id, &a.id, label)) {
                    o["stuck"] = json!(true);
                }
                o
            };
            let mine: Vec<&AccountRow> = rows.iter().filter(|r| r.account == a.id).collect();
            if mine.is_empty() {
                out.push(row(DEFAULT_LABEL, None, "none"));
            }
            // a new label's sign-in under way, or its failure, before it has a row
            let prefix = format!("{id}/{}/", a.id);
            let mut fresh: Vec<String> = self.accounts.pending().values().filter(|p| p.plugin == id && p.account == a.id).map(|p| p.label.clone()).collect();
            fresh.extend(self.accounts.errors().keys().filter_map(|k| k.strip_prefix(&prefix).map(str::to_owned)));
            fresh.sort();
            fresh.dedup();
            for l in fresh.into_iter().filter(|l| !mine.iter().any(|r| r.label == *l) && !(mine.is_empty() && l == DEFAULT_LABEL)) {
                out.push(row(&l, None, "none"));
            }
            let active = self.active_label(id, &a.id);
            for r in mine {
                let mut o = row(&r.label, Some(r.subject.as_str()).filter(|s| !s.is_empty()), &r.state);
                o["active"] = json!(active.as_deref() == Some(r.label.as_str()));
                if lacks_scopes(&a.scopes, &r.scopes) {
                    o["needs_access"] = json!(true);
                }
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
            // a stale or forged answer: the flow stays; the row says so
            self.accounts.errors().insert(token_ref(&p.plugin, &p.account, &p.label), ("The sign-in answer did not match. Sign in again.".to_owned(), false));
            self.emit(&p.plugin);
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
            // Google's token answer carries `scope`; the fallback is for others
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
        // tokens change only under the plugin's lock, as a refresh's do
        let lock = self.lock_of(&p.plugin);
        let _held = lock.lock().await;
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
        let url = Url::parse(endpoint).map_err(|e| Refused { why: e.to_string(), bad_client: false, after: After::Expire })?;
        let mut md = Map::new();
        md.insert("token_endpoint".into(), json!(endpoint));
        md.insert("token_endpoint_auth_methods_supported".into(), json!(methods));
        let fetcher = Fetcher::default();
        let asked = oauth::token_request(&fetcher, &url, Some(&md), params, Some(client), None);
        let answer = tokio::time::timeout(self.accounts.cfg().token_wait, asked).await;
        answer.unwrap_or_else(|_| Err(oauth::AuthError::Unreached("no answer in time".to_owned()))).map_err(|e| {
            let bad_client = matches!(&e, oauth::AuthError::OAuth { kind: oauth::Kind::InvalidClient | oauth::Kind::UnauthorizedClient, .. });
            Refused { why: js::slice_utf16(&e.message(), 300), bad_client, after: e.verdict().into() }
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
    async fn refresh_account(&self, r: &AccountRow) -> Result<Value, RefreshFailed> {
        let prev = self.tokens(&r.token_ref).await.ok_or("no stored tokens")?;
        let t = self.ask_refresh(r, &prev).await?;
        self.keep_refresh(r, &prev, &t).await
    }

    /// The provider's answer to a refresh of `prev`; nothing is stored yet.
    async fn ask_refresh(&self, r: &AccountRow, prev: &Value) -> Result<Map<String, Value>, RefreshFailed> {
        let rt = prev.get("refresh_token").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or("no refresh token")?.to_owned();
        let (endpoint, methods, client) = if r.provider == "google" {
            (self.accounts.cfg().google_token, vec!["client_secret_post".to_owned()], self.google_client().ok_or(NEEDS_GOOGLE_CLIENT)?)
        } else {
            let endpoint = prev.get("token_endpoint").and_then(Value::as_str).ok_or("no token endpoint")?.to_owned();
            let methods = prev.get("auth_methods").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
            (endpoint, methods, prev.get("client").and_then(Value::as_object).cloned().unwrap_or_default())
        };
        Ok(self.token_call(&endpoint, &methods, &client, vec![("grant_type".into(), "refresh_token".into()), ("refresh_token".into(), rt)]).await?)
    }

    /// Stores a refresh's answer; the new blob.
    async fn keep_refresh(&self, r: &AccountRow, prev: &Value, t: &Map<String, Value>) -> Result<Value, RefreshFailed> {
        let blob = self.blob(t, Some(prev));
        self.put_tokens(&r.token_ref, &blob).await.map_err(|e| RefreshFailed { why: e.message, after: After::Retry })?;
        if let Some(s) = scopes_of(t.get("scope")) {
            let _ = self.put_account(&AccountRow { scopes: s, ..r.clone() });
        }
        Ok(blob)
    }

    /// A reserved account call; -32601 is `plugin_invalid` (the plugin
    /// declares accounts, so it must answer both). A first-party plugin's
    /// carries `accounts`, as the rows are by now ([`Host::signed_in`]).
    async fn push(&self, conn: &Conn, plugin: &str, name: &str, mut args: Value) -> Result<(), Fail> {
        if self.told_the_list(plugin) {
            args["accounts"] = self.signed_in(plugin);
        }
        match self.reserved(conn, name, args, self.timeouts.reserved).await {
            Ok(_) => Ok(()),
            Err(Fail::Rpc(e)) if e.code == mcp::METHOD_NOT_FOUND => refuse("plugin_invalid", format!("{plugin} does not implement {name}")),
            Err(Fail::V1(e)) => refuse("refused", e.message),
            Err(e) => Err(e),
        }
    }

    /// A push to the running plugin (none running: the next spawn pushes);
    /// -32601 fails it.
    /// False when a running plugin didn't take it, so it's worth sending again.
    /// One that isn't running gets its tokens when it starts.
    async fn push_live(&self, plugin: &str, name: &str, args: Value) -> bool {
        let Ok(conn) = self.running_conn(plugin) else { return true };
        match self.push(&conn, plugin, name, args).await {
            Ok(()) => true,
            Err(Fail::V1(e)) if e.code == "plugin_invalid" => {
                self.stop_conn(plugin).await;
                self.set_state(plugin, "failed", Some("plugin_invalid"), &e.message);
                true
            }
            Err(e) => {
                eprintln!("  v1 {plugin}: {name}: {}", js::slice_utf16(e.message(), 200));
                false
            }
        }
    }

    /// After provision (the gate's connection): `buwiz.account.set` for
    /// each account's active label, refreshed first when it is within 5
    /// minutes of expiry (a refused refresh: expired, nothing pushed; one
    /// nobody answered: pushed as it stands, and tried again); the
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
            let mut unanswered = None;
            if self.due(&t) {
                match self.refresh_account(&r).await {
                    Ok(fresh) => t = fresh,
                    Err(e) => {
                        eprintln!("  v1 {id}: account {}/{}: refresh failed: {}", r.account, r.label, js::slice_utf16(&e.why, 200));
                        if e.after == After::Expire {
                            self.set_account_state(&r, "expired");
                            continue;
                        }
                        unanswered = Some(e.after);
                    }
                }
            }
            match self.push(conn, id, SET, set_args(&r, &t)).await {
                Ok(()) => match unanswered {
                    Some(after) => self.schedule_retry(&r, after),
                    None => self.schedule_refresh(&r, &t),
                },
                Err(Fail::V1(e)) if e.code == "plugin_invalid" => return Err(e),
                // the plugin has no token from this: the refresh pushes one
                Err(e) => {
                    eprintln!("  v1 {id}: {SET}: {}", js::slice_utf16(e.message(), 200));
                    self.schedule_retry(&r, unanswered.unwrap_or(After::Retry));
                }
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
        self.retrying().remove(&r.token_ref);
        self.refresh_in((expires * 1000.0 - REFRESH_EARLY_MS - self.clock.now()).max(0.0) as u64, r);
    }

    /// The next try of a refresh that didn't get the provider's answer.
    fn schedule_retry(&self, r: &AccountRow, after: After) {
        let now = self.clock.now();
        let tries = {
            let mut retrying = self.retrying();
            let (_, tries) = retrying.entry(r.token_ref.clone()).or_insert((now, 0));
            *tries += 1;
            *tries
        };
        self.refresh_in(retry_delay(after, tries), r);
    }

    fn retrying(&self) -> std::sync::MutexGuard<'_, HashMap<String, (f64, u32)>> {
        self.accounts.retrying.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `r`'s refresh has failed for [`REFRESH_STUCK_MS`], so its row says so.
    fn refresh_stuck(&self, r: &str) -> bool {
        self.retrying().get(r).is_some_and(|(since, _)| self.clock.now() - since >= REFRESH_STUCK_MS)
    }

    fn refresh_in(&self, delay: u64, r: &AccountRow) {
        self.clear_refresh(&r.token_ref);
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
    /// runs; a refusal is `expired` and `buwiz.account.clear`, and a refresh
    /// nobody answered changes nothing and tries again. The wait for the
    /// provider holds no lock: a stalled one must not block a switch or a
    /// sign-out, so what it answers counts only if the tokens are still those.
    async fn refresh_tick(&self, r: &AccountRow) {
        self.accounts.timers.lock().unwrap_or_else(|e| e.into_inner()).remove(&r.token_ref);
        if self.with_rt(&r.plugin, |rt| rt.state) != "running" {
            return;
        }
        let Some(r) = self.account_row(&r.plugin, &r.account, &r.label).filter(|x| x.state == "connected") else { return };
        let prev = self.tokens(&r.token_ref).await;
        let asked = match &prev {
            Some(prev) => self.ask_refresh(&r, prev).await,
            None => Err("no stored tokens".into()),
        };
        let lock = self.lock_of(&r.plugin);
        let _held = lock.lock().await;
        let Some(r) = self.account_row(&r.plugin, &r.account, &r.label).filter(|x| x.state == "connected") else { return };
        // a sign-in or a push's own refresh came first, and set the next timer
        if self.tokens(&r.token_ref).await != prev {
            return;
        }
        // every connected label stays fresh; only the active one is pushed
        let active = self.active_label(&r.plugin, &r.account).as_deref() == Some(r.label.as_str());
        let refreshed = match (asked, &prev) {
            (Ok(t), Some(prev)) => self.keep_refresh(&r, prev, &t).await,
            (Err(e), _) => Err(e),
            (Ok(_), None) => Err("no stored tokens".into()),
        };
        match refreshed {
            Ok(t) => {
                if !active || self.push_live(&r.plugin, SET, set_args(&r, &t)).await {
                    self.schedule_refresh(&r, &t);
                } else {
                    self.schedule_retry(&r, After::Retry);
                }
                self.account_changed(&r.plugin, &r.account, &r.label, "connected");
            }
            Err(e) if e.after != After::Expire => {
                eprintln!("  v1 {}: account {}/{}: refresh failed, trying again: {}", r.plugin, r.account, r.label, js::slice_utf16(&e.why, 200));
                self.schedule_retry(&r, e.after);
                if self.refresh_stuck(&r.token_ref) {
                    self.account_changed(&r.plugin, &r.account, &r.label, "connected");
                }
            }
            Err(e) => {
                eprintln!("  v1 {}: account {}/{}: refresh failed: {}", r.plugin, r.account, r.label, js::slice_utf16(&e.why, 200));
                let _ = self.sql(|db| db.execute("UPDATE v1_accounts SET state='expired' WHERE plugin=? AND account=? AND label=?", [&r.plugin, &r.account, &r.label]));
                self.account_changed(&r.plugin, &r.account, &r.label, "expired");
                // the active one's token was out. One that wasn't has news only
                // for a first-party plugin, whose list loses it
                if active || self.told_the_list(&r.plugin) {
                    self.push_live(&r.plugin, CLEAR, json!({"account": r.account, "label": r.label})).await;
                }
                if active {
                    self.promote_next(&r.plugin, &r.account).await;
                }
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
        // tokens change only under the plugin's lock: a refresh that has checked
        // them is done writing before this drops them
        let lock = self.lock_of(plugin);
        let _held = lock.lock().await;
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
            self.push_live(plugin, CLEAR, json!({"account": account, "label": label})).await;
            // the active one signed out: another connected label is
            if was_active {
                next = self.promote_next(plugin, account).await;
            }
        }
        let state = if remove { "removed" } else { "none" };
        self.account_changed(plugin, account, &label, state);
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
    fn only_a_stray_answer_backs_off() {
        let minutes = |after, tries| retry_delay(after, tries) / 60_000;
        assert_eq!([1, 2, 3, 40].map(|n| minutes(After::Retry, n)), [1, 1, 1, 1]);
        assert_eq!([1, 2, 3, 4, 5, 6, 7, 40].map(|n| minutes(After::BackOff, n)), [1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn only_a_refusal_ends_a_sign_in() {
        use oauth::{AuthError, Kind, Verdict};
        let oauth = |kind| AuthError::OAuth { kind, name: String::new(), message: String::new() };
        for kind in [Kind::InvalidGrant, Kind::InvalidClient, Kind::UnauthorizedClient, Kind::Other, Kind::Unknown] {
            assert_eq!(oauth(kind).verdict(), Verdict::Refused);
        }
        assert_eq!(AuthError::Plain("client_secret_basic authentication requires a client_secret".into()).verdict(), Verdict::Refused);
        for e in [AuthError::Unreached("gone".into()), oauth(Kind::Server), oauth(Kind::Busy)] {
            assert_eq!(e.verdict(), Verdict::Unanswered);
        }
        assert_eq!(oauth(Kind::Stray).verdict(), Verdict::Stray);
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
    fn a_sign_in_lacks_only_what_was_declared_and_not_granted() {
        let (read, modify) = ("gmail.readonly".to_owned(), "gmail.modify".to_owned());
        assert!(lacks_scopes(std::slice::from_ref(&modify), std::slice::from_ref(&read)));
        assert!(!lacks_scopes(std::slice::from_ref(&read), &[read.clone(), modify.clone()]));
        assert!(!lacks_scopes(&[modify], &[]), "none on record says nothing");
        assert!(!lacks_scopes(&[], &[read]));
    }

    /// A release build has no way to send a sign-in anywhere but Google.
    #[cfg(not(feature = "test-google"))]
    #[test]
    fn a_release_signs_in_at_google() {
        let cfg = AccountsConfig::from_env(8810).unwrap();
        assert_eq!((cfg.port, cfg.google_authorize.as_str(), cfg.google_token.as_str()), (8810, GOOGLE_AUTHORIZE, GOOGLE_TOKEN));
    }

    #[cfg(not(feature = "test-google"))]
    #[test]
    fn a_release_refuses_the_test_variable() {
        let refused = AccountsConfig::for_release(8810, Some(std::ffi::OsStr::new("http://127.0.0.1:9"))).err();
        assert!(refused.is_some_and(|e| e.contains("isn't a test-google build")));
        assert!(AccountsConfig::for_release(8810, None).is_ok());
    }

    #[cfg(feature = "test-google")]
    #[test]
    fn a_test_build_defaults_to_nowhere() {
        let cfg = AccountsConfig::default();
        assert!(![&cfg.google_authorize, &cfg.google_token].iter().any(|url| url.contains("google")));
    }

    #[cfg(feature = "test-google")]
    #[test]
    fn a_test_build_signs_in_at_the_stub_or_not_at_all() {
        use std::os::unix::ffi::OsStrExt as _;
        let value = |s: &str| std::ffi::OsStr::new(s).to_owned();
        let cfg = AccountsConfig::for_test(8810, Some(&value("http://127.0.0.1:9/"))).unwrap();
        assert_eq!((cfg.port, cfg.google_authorize.as_str(), cfg.google_token.as_str()), (8810, "http://127.0.0.1:9/o/oauth2/v2/auth", "http://127.0.0.1:9/token"));
        for base in ["https://accounts.example.com", "http://127.0.0.1.example.com", "http://127.0.0.1:9@attacker.example", "http://127.0.0.1:", "http://127.0.0.1:9/x", ""] {
            let refused = AccountsConfig::for_test(8810, Some(&value(base))).err();
            assert!(refused.is_some_and(|e| e.contains(TEST_GOOGLE_ENV)), "{base}");
        }
        assert!(AccountsConfig::for_test(8810, None).is_err(), "no value: never the real Google");
        assert!(AccountsConfig::for_test(8810, Some(std::ffi::OsStr::from_bytes(b"http://127.0.0.1:9\xff"))).is_err(), "not text");
    }

    /// The stub's consent: the authorize page redirects to the callback, and
    /// both are fetched in process, with no child left behind.
    #[cfg(feature = "test-google")]
    #[test]
    fn the_stub_sign_in_follows_the_redirect_to_the_callback() {
        use std::io::{Read as _, Write as _};
        let server = |reply: String| {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let (seen, heard) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = [0u8; 1024];
                let n = s.read(&mut buf).unwrap();
                seen.send(String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or_default().to_owned()).unwrap();
                s.write_all(reply.as_bytes()).unwrap();
            });
            (port, heard)
        };
        let (callback, called) = server("HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n".to_owned());
        let (consent, _consent_heard) = server(format!("HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1:{callback}/oauth/callback?code=c\r\ncontent-length: 0\r\n\r\n"));
        follow(&format!("http://127.0.0.1:{consent}/auth")).unwrap();
        assert_eq!(called.recv().unwrap(), "GET /oauth/callback?code=c HTTP/1.1");
        let (refusal, _refusal_heard) = server("HTTP/1.1 500 No\r\ncontent-length: 0\r\n\r\n".to_owned());
        assert_eq!(follow(&format!("http://127.0.0.1:{refusal}/auth")), Err("HTTP/1.1 500 No".to_owned()), "a failure is said, not lost");
        let (away, _away_heard) = server("HTTP/1.1 302 Found\r\nlocation: https://accounts.google.com/x\r\ncontent-length: 0\r\n\r\n".to_owned());
        assert_eq!(follow(&format!("http://127.0.0.1:{away}/auth")), Err("a hop off loopback was refused".to_owned()));
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
