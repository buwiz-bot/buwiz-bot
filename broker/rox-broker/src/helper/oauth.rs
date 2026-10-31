//! OAuth for remote MCP servers (plan 4.2), as the broker's Node did it:
//! the SDK's `auth()` (client/auth.js, 1.32.1) driving mcp.mjs's provider.
//! The MCP Authorization spec: RFC 9728 resource metadata, RFC 8414 (or
//! OpenID) server metadata, RFC 7591 registration, PKCE S256, the browser,
//! the callback, the code exchange, refresh. Storage is the row's `auth`
//! JSON column, read and written whole on every step as Node did
//! (`client_info`, `tokens`, `state`, `verifier`, `no_dcr`), with the SDK's
//! `issuer` stamp, so either broker reads what the other wrote.
//!
//! The redirect is the front door's fixed route,
//! `http://127.0.0.1:<port>/oauth/callback/<id>`; a pre-registered client
//! (`oauth_client`: id, secret, scope, extra authorize parameters) is the
//! user's own app and never registers. A flow the user starts opens the
//! browser (`open`, from PATH, as Node's execFile did) unless asked for the
//! URL; a transport's own re-auth on a 401 only records the URL.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use url::Url;

use super::http::{self, Req};

pub const PROTOCOL_VERSION: &str = super::mcp::PROTOCOL_VERSION;
const PENDING_FOR: Duration = Duration::from_secs(10 * 60);

/// Where a row's `auth` column lives (McpHost's app.db).
pub trait AuthStore: Send + Sync {
    /// The row's `auth` object (`{}` when empty or unreadable).
    fn load_auth(&self, id: &str) -> Value;
    fn save_auth(&self, id: &str, auth: &Value);
}

/// What OAuth needs of an `mcp_servers` row.
#[derive(Debug, Clone)]
pub struct Row {
    pub id: String,
    pub url: String,
    /// `oauth_client` when it names a `client_id`.
    pub fixed: Option<Map<String, Value>>,
    /// `auth` as read with the row (Node's `storedAuth(row)`).
    pub auth: Value,
}

impl Row {
    pub fn fixed_of(oauth_client: Option<&str>) -> Option<Map<String, Value>> {
        let v: Value = serde_json::from_str(oauth_client?).ok()?;
        let o = v.as_object()?;
        o.get("client_id").filter(|c| crate::js::truthy(c)).map(|_| o.clone())
    }
}

/// An OAuth error class (the SDK's OAUTH_ERRORS), or another failure.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    InvalidClient,
    UnauthorizedClient,
    InvalidGrant,
    /// `server_error`, an unknown code, or a body that isn't an OAuth error.
    Server,
    /// Any other code in OAUTH_ERRORS.
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuthError {
    /// `name`: the SDK's class (`InvalidGrantError`), what Node printed
    /// when the server gave no description.
    OAuth { kind: Kind, name: String, message: String },
    /// Not an OAuth error (network, metadata, schema).
    Plain(String),
}

impl AuthError {
    pub fn message(&self) -> String {
        match self {
            AuthError::OAuth { name, message, .. } if message.is_empty() => name.clone(),
            AuthError::OAuth { message, .. } | AuthError::Plain(message) => message.clone(),
        }
    }
}

fn plain(m: impl Into<String>) -> AuthError {
    AuthError::Plain(m.into())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    Authorized,
    Redirect,
}

/// The state Node kept in module maps: flows waiting for their callback,
/// the last authorize URL, rows asked not to open a tab, discovery caches.
#[derive(Default)]
pub struct OAuthState {
    pending: Mutex<HashMap<String, (Row, Instant)>>,
    last_authorize: Mutex<HashMap<String, String>>,
    no_open: Mutex<HashSet<String>>,
    scope_cache: tokio::sync::Mutex<HashMap<String, Option<String>>>,
    prm_cache: tokio::sync::Mutex<HashMap<String, bool>>,
}

/// mcp.mjs's provider for one row.
pub struct Provider<'a> {
    pub state: &'a OAuthState,
    pub store: &'a dyn AuthStore,
    pub row: &'a Row,
    pub port: u16,
    /// A transport's re-auth: never opens a tab.
    pub background: bool,
}

pub fn fixed_redirect_url(port: u16, id: &str) -> String {
    format!("http://127.0.0.1:{port}/oauth/callback/{}", encode_component(id))
}

/// `encodeURIComponent`.
fn encode_component(s: &str) -> String {
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

impl Provider<'_> {
    fn stored(&self) -> Map<String, Value> {
        self.store.load_auth(&self.row.id).as_object().cloned().unwrap_or_default()
    }

    fn save(&self, key: &str, value: Value) {
        let mut a = self.stored();
        a.insert(key.to_owned(), value);
        self.store.save_auth(&self.row.id, &Value::Object(a));
    }

    pub fn redirect_url(&self) -> String {
        fixed_redirect_url(self.port, &self.row.id)
    }

    fn secret(&self) -> Option<&Value> {
        self.row.fixed.as_ref().and_then(|f| f.get("client_secret")).filter(|s| crate::js::truthy(s))
    }

    fn client_metadata(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("client_name".into(), json!("Buwiz Bot"));
        m.insert("redirect_uris".into(), json!([self.redirect_url()]));
        m.insert("grant_types".into(), json!(["authorization_code", "refresh_token"]));
        m.insert("response_types".into(), json!(["code"]));
        m.insert("token_endpoint_auth_method".into(), json!(if self.secret().is_some() { "client_secret_basic" } else { "none" }));
        if let Some(s) = self.row.fixed.as_ref().and_then(|f| f.get("scope")).filter(|s| crate::js::truthy(s)) {
            m.insert("scope".into(), s.clone());
        }
        m
    }

    fn client_information(&self) -> Option<Map<String, Value>> {
        if let Some(f) = &self.row.fixed {
            let mut m = Map::new();
            m.insert("client_id".into(), f.get("client_id").cloned().unwrap_or(Value::Null));
            if let Some(s) = self.secret() {
                m.insert("client_secret".into(), s.clone());
            }
            return Some(m);
        }
        let info = self.stored().get("client_info").and_then(Value::as_object).cloned()?;
        // registered for another redirect (an old port): register again
        match info.get("redirect_uris").and_then(Value::as_array) {
            Some(uris) if !uris.iter().any(|u| u.as_str() == Some(&self.redirect_url())) => None,
            _ => Some(info),
        }
    }

    fn save_client_information(&self, info: Map<String, Value>) {
        if self.row.fixed.is_none() {
            self.save("client_info", Value::Object(info));
        }
    }

    fn tokens(&self) -> Option<Map<String, Value>> {
        self.stored().get("tokens").and_then(Value::as_object).cloned()
    }

    pub fn access_token(&self) -> Option<String> {
        self.tokens().and_then(|t| t.get("access_token").map(crate::js::string_of))
    }

    fn save_tokens(&self, t: Map<String, Value>) {
        self.save("tokens", Value::Object(t));
    }

    fn redirect_to_authorization(&self, mut url: Url) {
        if let Some(extra) = self.row.fixed.as_ref().and_then(|f| f.get("extra")).and_then(Value::as_object) {
            for (k, v) in crate::js::ordered(extra) {
                set_param(&mut url, k, &crate::js::string_of(v));
            }
        }
        let u = url.to_string();
        self.state.last_authorize.lock().unwrap_or_else(|e| e.into_inner()).insert(self.row.id.clone(), u.clone());
        eprintln!("  mcp {}: authorize {}", self.row.id, crate::js::slice_utf16(&u, 400));
        let no_open = self.state.no_open.lock().unwrap_or_else(|e| e.into_inner()).contains(&self.row.id);
        if !self.background && !no_open {
            open_url(&u);
        }
    }

    fn new_state(&self) -> String {
        let st: String = rox_v1::keys::random::<16>().iter().map(|b| format!("{b:02x}")).collect();
        self.save("state", json!(st));
        st
    }

    fn code_verifier(&self) -> String {
        self.stored().get("verifier").map(crate::js::string_of).filter(|v| !v.is_empty()).unwrap_or_default()
    }
}

/// The system's way to open a URL (`open` on macOS, as Node's execFile ran
/// it: looked up on PATH).
fn open_url(u: &str) {
    let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    if let Ok(mut child) = std::process::Command::new(cmd).arg(u).stdin(std::process::Stdio::null()).spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// `URLSearchParams.set`: the first `name` takes `value` in place, the
/// others go; appended when absent.
pub fn set_param(url: &mut Url, name: &str, value: &str) {
    let pairs: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut done = false;
    for (k, v) in pairs {
        if k == name {
            if !done {
                out.push((k, value.to_owned()));
                done = true;
            }
        } else {
            out.push((k, v));
        }
    }
    if !done {
        out.push((name.to_owned(), value.to_owned()));
    }
    url.query_pairs_mut().clear().extend_pairs(out);
}

fn append_param(url: &mut Url, name: &str, value: &str) {
    url.query_pairs_mut().append_pair(name, value);
}

// ── discovery ────────────────────────────────────────────────────────────

/// How a flow's requests go out: the transport's `fetchWithInit` adds the
/// row's headers to every one (a request's own win); a user's sign-in uses
/// plain fetch.
#[derive(Clone, Default)]
pub struct Fetcher {
    pub init: Vec<(String, String)>,
}

impl Fetcher {
    async fn fetch(&self, mut req: Req) -> Result<http::Resp, String> {
        let own = std::mem::take(&mut req.headers);
        for (k, v) in self.init.iter().chain(own.iter()) {
            req.set(k, v);
        }
        http::fetch(req).await
    }

    /// `fetchWithCorsRetry`: a network failure is tried once more without
    /// headers, then counts as no answer.
    async fn get_retrying(&self, url: &Url, headers: &[(&str, &str)]) -> Option<http::Resp> {
        let mut req = Req::get(url.clone());
        for (k, v) in headers {
            req.set(k, v);
        }
        match self.fetch(req).await {
            Ok(r) => Some(r),
            Err(_) => self.fetch(Req::get(url.clone())).await.ok(),
        }
    }
}

/// `buildWellKnownPath`.
fn well_known_path(kind: &str, pathname: &str, prepend: bool) -> String {
    let p = pathname.strip_suffix('/').unwrap_or(pathname);
    if prepend {
        format!("{p}/.well-known/{kind}")
    } else {
        format!("/.well-known/{kind}{p}")
    }
}

/// `discoverMetadataWithFallback` for resource metadata.
async fn prm_response(f: &Fetcher, server: &Url, metadata_url: Option<&Url>) -> Option<http::Resp> {
    let headers = [("MCP-Protocol-Version", PROTOCOL_VERSION)];
    let url = match metadata_url {
        Some(u) => u.clone(),
        None => {
            let mut u = server.join(&well_known_path("oauth-protected-resource", server.path(), false)).ok()?;
            u.set_query(server.query());
            u
        }
    };
    let res = f.get_retrying(&url, &headers).await;
    let fallback = match &res {
        None => true,
        Some(r) => !r.ok() && r.status < 500 && server.path() != "/",
    };
    if metadata_url.is_none() && fallback {
        let root = server.join("/.well-known/oauth-protected-resource").ok()?;
        return f.get_retrying(&root, &headers).await;
    }
    res
}

/// A string that parses as a URL and isn't javascript:, data: or vbscript:
/// (SafeUrlSchema).
fn safe_url(v: &Value) -> bool {
    v.as_str().and_then(|s| Url::parse(s).ok()).is_some_and(|u| !["javascript", "data", "vbscript"].contains(&u.scheme()))
}

fn strings(v: Option<&Value>) -> bool {
    v.is_none_or(|v| v.as_array().is_some_and(|a| a.iter().all(Value::is_string)))
}

/// `discoverOAuthProtectedResourceMetadata`.
pub async fn discover_prm(f: &Fetcher, server: &Url, metadata_url: Option<&Url>) -> Result<Map<String, Value>, String> {
    let res = prm_response(f, server, metadata_url).await;
    let Some(res) = res.filter(|r| r.status != 404) else {
        return Err("Resource server does not implement OAuth 2.0 Protected Resource Metadata.".into());
    };
    if !res.ok() {
        return Err(format!("HTTP {} trying to load well-known OAuth protected resource metadata.", res.status));
    }
    let v: Value = serde_json::from_str(&res.text().await?).map_err(|e| e.to_string())?;
    let o = v.as_object().cloned().ok_or("resource metadata is not an object")?;
    let resource_ok = o.get("resource").and_then(Value::as_str).is_some_and(|s| Url::parse(s).is_ok());
    let servers_ok = o.get("authorization_servers").is_none_or(|a| a.as_array().is_some_and(|a| a.iter().all(safe_url)));
    if !resource_ok || !servers_ok || !strings(o.get("scopes_supported")) {
        return Err("Invalid OAuth protected resource metadata".into());
    }
    Ok(o)
}

/// `buildDiscoveryUrls`: (url, is OAuth rather than OpenID).
pub fn discovery_urls(as_url: &Url) -> Vec<(Url, bool)> {
    let origin = as_url.origin().ascii_serialization();
    let at = |p: String| Url::parse(&format!("{origin}{p}")).ok();
    let mut out = Vec::new();
    if as_url.path() == "/" {
        out.extend(at("/.well-known/oauth-authorization-server".into()).map(|u| (u, true)));
        out.extend(at("/.well-known/openid-configuration".into()).map(|u| (u, false)));
        return out;
    }
    let p = as_url.path().strip_suffix('/').unwrap_or(as_url.path()).to_owned();
    out.extend(at(format!("/.well-known/oauth-authorization-server{p}")).map(|u| (u, true)));
    out.extend(at(format!("/.well-known/openid-configuration{p}")).map(|u| (u, false)));
    out.extend(at(format!("{p}/.well-known/openid-configuration")).map(|u| (u, false)));
    out
}

/// `discoverAuthorizationServerMetadata`.
pub async fn discover_as(f: &Fetcher, as_url: &Url) -> Result<Option<Map<String, Value>>, String> {
    let headers = [("MCP-Protocol-Version", PROTOCOL_VERSION), ("Accept", "application/json")];
    for (url, oauth) in discovery_urls(as_url) {
        let Some(res) = f.get_retrying(&url, &headers).await else { continue };
        if !res.ok() {
            if res.status < 500 {
                continue;
            }
            return Err(format!("HTTP {} trying to load {} metadata from {url}", res.status, if oauth { "OAuth" } else { "OpenID provider" }));
        }
        let v: Value = serde_json::from_str(&res.text().await?).map_err(|e| e.to_string())?;
        let o = v.as_object().cloned().ok_or("server metadata is not an object")?;
        let mut ok = o.get("issuer").is_some_and(Value::is_string)
            && o.get("authorization_endpoint").is_some_and(safe_url)
            && o.get("token_endpoint").is_some_and(safe_url)
            && o.get("registration_endpoint").is_none_or(safe_url)
            && o.get("response_types_supported").is_some_and(|a| a.as_array().is_some_and(|a| a.iter().all(Value::is_string)))
            && strings(o.get("scopes_supported"))
            && strings(o.get("token_endpoint_auth_methods_supported"))
            && strings(o.get("code_challenge_methods_supported"));
        if !oauth {
            ok = ok && o.get("jwks_uri").is_some_and(safe_url) && strings(o.get("subject_types_supported")) && o.get("subject_types_supported").is_some();
        }
        if !ok {
            return Err(format!("Invalid {} metadata from {url}", if oauth { "OAuth" } else { "OpenID provider" }));
        }
        return Ok(Some(o));
    }
    Ok(None)
}

/// `discardIfIssuerMismatch` (with `issuersMatch`).
fn issuer_checked(stored: Option<Map<String, Value>>, issuer: &str) -> Option<Map<String, Value>> {
    let mut s = stored?;
    match s.get("issuer").cloned() {
        Some(Value::String(i)) => issuers_match(&i, issuer).then_some(s),
        None | Some(Value::Null) => Some(s),
        Some(_) => {
            s.remove("issuer");
            Some(s)
        }
    }
}

pub fn issuers_match(a: &str, b: &str) -> bool {
    let (x, y) = match (Url::parse(a), Url::parse(b)) {
        (Ok(x), Ok(y)) => (x.to_string(), y.to_string()),
        _ => (a.to_owned(), b.to_owned()),
    };
    x == y || x.strip_suffix('/') == Some(y.as_str()) || y.strip_suffix('/') == Some(x.as_str())
}

/// `checkResourceAllowed`.
fn resource_allowed(requested: &Url, configured: &Url) -> bool {
    if requested.origin() != configured.origin() || requested.path().len() < configured.path().len() {
        return false;
    }
    let slash = |p: &str| if p.ends_with('/') { p.to_owned() } else { format!("{p}/") };
    slash(requested.path()).starts_with(&slash(configured.path()))
}

/// `selectClientAuthMethod`.
pub fn client_auth_method(info: &Map<String, Value>, supported: &[String]) -> String {
    let secret = info.contains_key("client_secret");
    let known = ["client_secret_basic", "client_secret_post", "none"];
    if let Some(m) = info.get("token_endpoint_auth_method").and_then(Value::as_str) {
        if known.contains(&m) && (supported.is_empty() || supported.iter().any(|s| s == m)) {
            return m.to_owned();
        }
    }
    let has = |m: &str| supported.iter().any(|s| s == m);
    if supported.is_empty() {
        return if secret { "client_secret_basic" } else { "none" }.into();
    }
    if secret && has("client_secret_basic") {
        return "client_secret_basic".into();
    }
    if secret && has("client_secret_post") {
        return "client_secret_post".into();
    }
    if has("none") {
        return "none".into();
    }
    if secret { "client_secret_post" } else { "none" }.into()
}

/// pkce-challenge's verifier (43 characters of the unreserved set, from
/// unbiased bytes) and its S256 challenge.
pub fn pkce() -> (String, String) {
    const MASK: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";
    let cutoff = 256 - 256 % MASK.len();
    let mut v = String::new();
    while v.len() < 43 {
        for b in rox_v1::keys::random::<64>() {
            if (b as usize) < cutoff && v.len() < 43 {
                v.push(MASK[b as usize % MASK.len()] as char);
            }
        }
    }
    let c = challenge_of(&v);
    (v, c)
}

pub fn challenge_of(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// `parseErrorResponse`.
async fn error_of(res: http::Resp) -> AuthError {
    let status = res.status;
    let redirect = http::redirect_target(&res);
    let body = res.text().await.unwrap_or_default();
    let parsed: Option<Value> = serde_json::from_str(&body).ok();
    let o = parsed.as_ref().and_then(Value::as_object);
    let valid = o.is_some_and(|o| {
        o.get("error").is_some_and(Value::is_string) && o.get("error_description").is_none_or(Value::is_string) && o.get("error_uri").is_none_or(Value::is_string)
    });
    if let (true, Some(o)) = (valid, o) {
        let code = o.get("error").and_then(Value::as_str).unwrap_or_default();
        let kind = match code {
            "invalid_client" => Kind::InvalidClient,
            "unauthorized_client" => Kind::UnauthorizedClient,
            "invalid_grant" => Kind::InvalidGrant,
            "invalid_request" | "unsupported_grant_type" | "invalid_scope" | "access_denied" | "temporarily_unavailable" | "unsupported_response_type"
            | "unsupported_token_type" | "invalid_token" | "method_not_allowed" | "too_many_requests" | "invalid_client_metadata" | "insufficient_scope"
            | "invalid_target" => Kind::Other,
            _ => Kind::Server,
        };
        let message = o.get("error_description").and_then(Value::as_str).unwrap_or_default().to_owned();
        let name = match kind {
            Kind::Server => "ServerError".to_owned(),
            _ => code.split('_').map(|w| w[..1].to_uppercase() + &w[1..]).collect::<String>() + "Error",
        };
        return AuthError::OAuth { kind, name, message };
    }
    let text = match redirect {
        Some(t) => format!("Redirect to {} not followed", t.as_str()),
        None => format!("Invalid OAuth error response: {}. Raw body: {body}", if parsed.is_some() { "ZodError" } else { "SyntaxError" }),
    };
    AuthError::OAuth { kind: Kind::Server, name: "ServerError".into(), message: format!("HTTP {status}: {text}") }
}

/// A token response, as OAuthTokensSchema keeps it (its fields, in order;
/// `expires_in` coerced to a number).
fn tokens_of(v: &Value) -> Result<Map<String, Value>, AuthError> {
    let o = v.as_object().ok_or_else(|| plain("Invalid token response"))?;
    let mut t = Map::new();
    for k in ["access_token", "id_token", "token_type", "expires_in", "scope", "refresh_token"] {
        let Some(x) = o.get(k) else { continue };
        let x = if k == "expires_in" {
            match x {
                Value::Number(_) => x.clone(),
                Value::String(s) => s.trim().parse::<f64>().ok().and_then(serde_json::Number::from_f64).map(Value::Number).ok_or_else(|| plain("Invalid token response"))?,
                Value::Null => continue,
                _ => return Err(plain("Invalid token response")),
            }
        } else if x.is_string() {
            x.clone()
        } else {
            return Err(plain("Invalid token response"));
        };
        t.insert(k.into(), x);
    }
    if !t.contains_key("access_token") || !t.contains_key("token_type") {
        return Err(plain("Invalid token response"));
    }
    Ok(t)
}

/// A registration response, as OAuthClientInformationFullSchema keeps it.
fn registration_of(v: &Value) -> Result<Map<String, Value>, AuthError> {
    let o = v.as_object().ok_or_else(|| plain("Invalid registration response"))?;
    let uris_ok = o.get("redirect_uris").and_then(Value::as_array).is_some_and(|a| a.iter().all(safe_url));
    if !uris_ok || !o.get("client_id").is_some_and(Value::is_string) {
        return Err(plain("Invalid registration response"));
    }
    let mut out = Map::new();
    for k in [
        "redirect_uris", "token_endpoint_auth_method", "grant_types", "response_types", "client_name", "client_uri", "logo_uri", "scope", "contacts", "tos_uri",
        "policy_uri", "jwks_uri", "jwks", "software_id", "software_version", "software_statement", "client_id", "client_secret", "client_id_issued_at",
        "client_secret_expires_at",
    ] {
        match o.get(k) {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) if s.is_empty() && (k == "logo_uri" || k == "tos_uri") => {}
            Some(x) => {
                out.insert(k.into(), x.clone());
            }
        }
    }
    Ok(out)
}

fn with_issuer(mut m: Map<String, Value>, issuer: &str) -> Map<String, Value> {
    m.remove("issuer");
    m.insert("issuer".into(), json!(issuer));
    m
}

fn form(pairs: &[(String, String)]) -> Vec<u8> {
    url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish().into_bytes()
}

/// `executeTokenRequest`.
async fn token_request(
    f: &Fetcher,
    as_url: &Url,
    metadata: Option<&Map<String, Value>>,
    mut params: Vec<(String, String)>,
    info: Option<&Map<String, Value>>,
    resource: Option<&str>,
) -> Result<Map<String, Value>, AuthError> {
    let token_url = match metadata.and_then(|m| m.get("token_endpoint")).and_then(Value::as_str) {
        Some(t) => Url::parse(t).map_err(|e| plain(e.to_string()))?,
        None => as_url.join("/token").map_err(|e| plain(e.to_string()))?,
    };
    let mut req = Req::post(token_url, Vec::new()).header("Content-Type", "application/x-www-form-urlencoded").header("Accept", "application/json");
    if let Some(r) = resource {
        params.retain(|(k, _)| k != "resource");
        params.push(("resource".into(), r.into()));
    }
    if let Some(info) = info {
        let supported: Vec<String> =
            metadata.and_then(|m| m.get("token_endpoint_auth_methods_supported")).and_then(Value::as_array).map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        let id = info.get("client_id").map(crate::js::string_of).unwrap_or_default();
        let secret = info.get("client_secret").map(crate::js::string_of);
        let mut set = |k: &str, v: String| {
            params.retain(|(x, _)| x != k);
            params.push((k.into(), v));
        };
        match client_auth_method(info, &supported).as_str() {
            "client_secret_basic" => {
                let Some(s) = secret.filter(|s| !s.is_empty()) else { return Err(plain("client_secret_basic authentication requires a client_secret")) };
                let b = base64::engine::general_purpose::STANDARD.encode(format!("{id}:{s}"));
                req.set("Authorization", &format!("Basic {b}"));
            }
            "client_secret_post" => {
                set("client_id", id);
                if let Some(s) = secret.filter(|s| !s.is_empty()) {
                    set("client_secret", s);
                }
            }
            _ => set("client_id", id),
        }
    }
    req.body = Some(form(&params));
    let res = f.fetch(req).await.map_err(plain)?;
    if !res.ok() {
        return Err(error_of(res).await);
    }
    let v: Value = serde_json::from_str(&res.text().await.map_err(plain)?).map_err(|e| plain(e.to_string()))?;
    tokens_of(&v)
}

/// `registerClient`.
async fn register(f: &Fetcher, as_url: &Url, metadata: Option<&Map<String, Value>>, client: &Map<String, Value>, scope: Option<&str>) -> Result<Map<String, Value>, AuthError> {
    let url = match metadata {
        Some(m) => {
            let Some(r) = m.get("registration_endpoint").and_then(Value::as_str) else {
                return Err(plain("Incompatible auth server: does not support dynamic client registration"));
            };
            Url::parse(r).map_err(|e| plain(e.to_string()))?
        }
        None => as_url.join("/register").map_err(|e| plain(e.to_string()))?,
    };
    let mut body = client.clone();
    if let Some(s) = scope {
        body.insert("scope".into(), json!(s));
    }
    let req = Req::post(url, serde_json::to_vec(&Value::Object(body)).unwrap_or_default()).header("Content-Type", "application/json");
    let res = f.fetch(req).await.map_err(plain)?;
    if !res.ok() {
        return Err(error_of(res).await);
    }
    let v: Value = serde_json::from_str(&res.text().await.map_err(plain)?).map_err(|e| plain(e.to_string()))?;
    registration_of(&v)
}

/// What `auth()` is asked.
#[derive(Default, Clone)]
pub struct AuthOpts {
    pub server_url: String,
    pub code: Option<String>,
    pub scope: Option<String>,
    pub resource_metadata_url: Option<Url>,
    pub fetcher: Fetcher,
}

/// The SDK's `auth()`: once more after an invalid client or grant (the
/// provider has no invalidateCredentials, so the second run sees the same
/// storage).
pub async fn auth(p: &Provider<'_>, o: &AuthOpts) -> Result<Outcome, AuthError> {
    match auth_internal(p, o).await {
        Err(AuthError::OAuth { kind: Kind::InvalidClient | Kind::UnauthorizedClient | Kind::InvalidGrant, .. }) => auth_internal(p, o).await,
        r => r,
    }
}

async fn auth_internal(p: &Provider<'_>, o: &AuthOpts) -> Result<Outcome, AuthError> {
    let server = Url::parse(&o.server_url).map_err(|e| plain(e.to_string()))?;
    let f = &o.fetcher;
    // discoverOAuthServerInfo
    let prm = discover_prm(f, &server, o.resource_metadata_url.as_ref()).await.ok();
    let as_str = prm
        .as_ref()
        .and_then(|m| m.get("authorization_servers"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| server.join("/").map(|u| u.to_string()).unwrap_or_default());
    let as_url = Url::parse(&as_str).map_err(|e| plain(e.to_string()))?;
    let metadata = discover_as(f, &as_url).await.map_err(plain)?;
    let issuer = as_str.clone();
    // selectResourceURL: the metadata's resource, verbatim
    let resource = match &prm {
        None => None,
        Some(m) => {
            let configured = m.get("resource").and_then(Value::as_str).unwrap_or_default().to_owned();
            let mut requested = server.clone();
            requested.set_fragment(None);
            let c = Url::parse(&configured).map_err(|e| plain(e.to_string()))?;
            if !resource_allowed(&requested, &c) {
                return Err(plain(format!("Protected resource {configured} does not match expected {requested} (or origin)")));
            }
            Some(configured)
        }
    };
    let prm_scope = prm.as_ref().and_then(|m| m.get("scopes_supported")).and_then(Value::as_array).map(|a| a.iter().map(crate::js::string_of).collect::<Vec<_>>().join(" "));
    let client_meta = p.client_metadata();
    let scope = o
        .scope
        .clone()
        .filter(|s| !s.is_empty())
        .or(prm_scope.filter(|s| !s.is_empty()))
        .or_else(|| client_meta.get("scope").and_then(Value::as_str).map(str::to_owned).filter(|s| !s.is_empty()));

    let mut info = issuer_checked(p.client_information(), &issuer);
    let unstamped = info.clone().filter(|i| i.get("issuer").is_none_or(Value::is_null));
    let bind = |p: &Provider<'_>| {
        if let Some(u) = &unstamped {
            p.save_client_information(with_issuer(u.clone(), &issuer));
        }
    };
    if info.is_none() {
        if o.code.is_some() {
            return Err(plain("Existing OAuth client information is required when exchanging an authorization code"));
        }
        let full = register(f, &as_url, metadata.as_ref(), &client_meta, scope.as_deref()).await?;
        let stamped = with_issuer(full, &issuer);
        p.save_client_information(stamped.clone());
        info = Some(stamped);
    }
    let info = info.expect("registered above");

    if let Some(code) = &o.code {
        // fetchToken: the client information as stored now, checked again
        let stored = p.client_information();
        let checked = issuer_checked(stored.clone(), &issuer);
        if stored.is_some() && checked.is_none() {
            let at = stored.and_then(|s| s.get("issuer").cloned()).map(|v| crate::js::string_of(&v)).unwrap_or_default();
            return Err(plain(format!(
                "OAuth client information is bound to authorization server {at} and is not presented to {issuer}. Clear the stored client information, or correct `expectedIssuer`, if the authorization server has moved."
            )));
        }
        let params = vec![
            ("grant_type".into(), "authorization_code".into()),
            ("code".into(), code.clone()),
            ("code_verifier".into(), p.code_verifier()),
            ("redirect_uri".into(), p.redirect_url()),
        ];
        let tokens = token_request(f, &as_url, metadata.as_ref(), params, checked.as_ref().or(Some(&info)), resource.as_deref()).await?;
        bind(p);
        p.save_tokens(with_issuer(tokens, &issuer));
        return Ok(Outcome::Authorized);
    }

    let tokens = issuer_checked(p.tokens(), &issuer);
    if let Some(refresh) = tokens.as_ref().and_then(|t| t.get("refresh_token")).map(crate::js::string_of).filter(|r| !r.is_empty()) {
        let params = vec![("grant_type".into(), "refresh_token".into()), ("refresh_token".into(), refresh.clone())];
        match token_request(f, &as_url, metadata.as_ref(), params, Some(&info), resource.as_deref()).await {
            Ok(new) => {
                // the old refresh token stays unless a new one came
                let mut t = Map::new();
                t.insert("refresh_token".into(), json!(refresh));
                for (k, v) in new {
                    t.insert(k, v);
                }
                bind(p);
                p.save_tokens(with_issuer(t, &issuer));
                return Ok(Outcome::Authorized);
            }
            // a server error or no OAuth error at all: go on to sign in
            Err(AuthError::Plain(_)) | Err(AuthError::OAuth { kind: Kind::Server, .. }) => {}
            Err(e) => return Err(e),
        }
    }

    let state = p.new_state();
    // startAuthorization
    let mut url = match metadata.as_ref() {
        Some(m) => {
            let endpoint = m.get("authorization_endpoint").and_then(Value::as_str).unwrap_or_default();
            let types = m.get("response_types_supported").and_then(Value::as_array);
            if !types.is_some_and(|t| t.iter().any(|x| x == "code")) {
                return Err(plain("Incompatible auth server: does not support response type code"));
            }
            if let Some(methods) = m.get("code_challenge_methods_supported").and_then(Value::as_array) {
                if !methods.iter().any(|x| x == "S256") {
                    return Err(plain("Incompatible auth server: does not support code challenge method S256"));
                }
            }
            Url::parse(endpoint).map_err(|e| plain(e.to_string()))?
        }
        None => as_url.join("/authorize").map_err(|e| plain(e.to_string()))?,
    };
    let (verifier, challenge) = pkce();
    let client_id = info.get("client_id").map(crate::js::string_of).unwrap_or_default();
    set_param(&mut url, "response_type", "code");
    set_param(&mut url, "client_id", &client_id);
    set_param(&mut url, "code_challenge", &challenge);
    set_param(&mut url, "code_challenge_method", "S256");
    set_param(&mut url, "redirect_uri", &p.redirect_url());
    set_param(&mut url, "state", &state);
    if let Some(s) = &scope {
        set_param(&mut url, "scope", s);
        if s.contains("offline_access") {
            append_param(&mut url, "prompt", "consent");
        }
    }
    if let Some(r) = &resource {
        set_param(&mut url, "resource", r);
    }
    p.save("verifier", json!(verifier));
    p.redirect_to_authorization(url);
    Ok(Outcome::Redirect)
}

// ── the flows mcp.mjs ran ────────────────────────────────────────────────

impl OAuthState {
    /// `offlineScope`: the resource's scopes plus offline_access when its
    /// authorization server offers it (else none, the SDK's own choice).
    async fn offline_scope(&self, url: &str) -> Option<String> {
        let mut cache = self.scope_cache.lock().await;
        if let Some(s) = cache.get(url) {
            return s.clone();
        }
        let s = async {
            let server = Url::parse(url).ok()?;
            let f = Fetcher::default();
            let prm = discover_prm(&f, &server, None).await.ok();
            let first = prm.as_ref().and_then(|m| m.get("authorization_servers")).and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str).map(str::to_owned);
            let as_url = Url::parse(&first.unwrap_or_else(|| server.join("/").map(|u| u.to_string()).unwrap_or_default())).ok()?;
            let m = discover_as(&f, &as_url).await.ok()??;
            if !m.get("scopes_supported").and_then(Value::as_array).is_some_and(|a| a.iter().any(|s| s == "offline_access")) {
                return None;
            }
            let mut all: Vec<String> = prm.as_ref().and_then(|m| m.get("scopes_supported")).and_then(Value::as_array).map(|a| a.iter().map(crate::js::string_of).collect()).unwrap_or_default();
            all.push("offline_access".into());
            let mut seen = HashSet::new();
            all.retain(|s| seen.insert(s.clone()));
            Some(all.join(" "))
        }
        .await;
        cache.insert(url.to_owned(), s.clone());
        s
    }

    /// `wantsOAuth`: does the server publish resource metadata naming an
    /// authorization server?
    pub async fn wants_oauth(&self, url: &str) -> bool {
        let mut cache = self.prm_cache.lock().await;
        if let Some(w) = cache.get(url) {
            return *w;
        }
        let w = match Url::parse(url) {
            Ok(u) => discover_prm(&Fetcher::default(), &u, None)
                .await
                .is_ok_and(|m| m.get("authorization_servers").and_then(Value::as_array).is_some_and(|a| !a.is_empty())),
            Err(_) => false,
        };
        cache.insert(url.to_owned(), w);
        w
    }

    /// `beginAuth`: {ok, authorized} or {ok, redirected, authorizeUrl?};
    /// Err is the message the route sends.
    pub async fn begin(&self, store: &dyn AuthStore, port: u16, row: &Row, open: bool) -> Result<Value, String> {
        if row.url.is_empty() {
            return Err("not a remote server".into());
        }
        let p = Provider { state: self, store, row, port, background: false };
        self.last_authorize.lock().unwrap_or_else(|e| e.into_inner()).remove(&row.id);
        if !open {
            self.no_open.lock().unwrap_or_else(|e| e.into_inner()).insert(row.id.clone());
        }
        let fixed_scope = row.fixed.as_ref().and_then(|f| f.get("scope")).filter(|s| crate::js::truthy(s)).map(crate::js::string_of);
        let scope = match fixed_scope {
            Some(s) => Some(s),
            None => self.offline_scope(&row.url).await,
        };
        let result = auth(&p, &AuthOpts { server_url: row.url.clone(), scope, ..Default::default() }).await;
        self.no_open.lock().unwrap_or_else(|e| e.into_inner()).remove(&row.id);
        let outcome = match result {
            Ok(o) => o,
            Err(e) => {
                let msg = e.message();
                let lower = msg.to_lowercase();
                if row.fixed.is_none() && (lower.contains("dynamic client registration") || lower.contains("registration_endpoint") || lower.contains("does not support")) {
                    let mut a = row.auth.as_object().cloned().unwrap_or_default();
                    a.insert("no_dcr".into(), json!(true));
                    store.save_auth(&row.id, &Value::Object(a));
                    return Err("This app's sign-in has no automatic client registration — add your own OAuth Client ID in Setup".into());
                }
                return Err(msg);
            }
        };
        if outcome == Outcome::Authorized {
            return Ok(json!({"ok": true, "authorized": true}));
        }
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(row.id.clone(), (row.clone(), Instant::now() + PENDING_FOR));
        let mut out = json!({"ok": true, "redirected": true});
        if !open {
            out["authorizeUrl"] = self.last_authorize.lock().unwrap_or_else(|e| e.into_inner()).get(&row.id).map(|u| json!(u)).unwrap_or(Value::Null);
        }
        Ok(out)
    }

    fn pending_row(&self, id: &str) -> Option<Row> {
        let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        p.retain(|_, (_, until)| *until > Instant::now());
        p.get(id).map(|(r, _)| r.clone())
    }

    /// The callback (`handleOAuthCallback`): "refused" (400), "failed" (the
    /// failure page, with why), or "connected".
    pub async fn finish(&self, store: &dyn AuthStore, port: u16, id: &str, code: Option<&str>, state: Option<&str>, error: Option<&str>) -> (&'static str, String) {
        if id.is_empty() || (code.is_none() && error.is_none()) {
            return ("refused", String::new());
        }
        let Some(row) = self.pending_row(id) else { return ("refused", String::new()) };
        let want = store.load_auth(id).get("state").map(crate::js::string_of).filter(|s| !s.is_empty());
        if let (Some(_), Some(w)) = (code, &want) {
            if state != Some(w.as_str()) {
                return ("refused", String::new());
            }
        }
        if error.is_some() {
            self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
            return ("failed", String::new());
        }
        let p = Provider { state: self, store, row: &row, port, background: false };
        match auth(&p, &AuthOpts { server_url: row.url.clone(), code: code.map(str::to_owned), ..Default::default() }).await {
            Ok(_) => {
                self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
                ("connected", String::new())
            }
            Err(e) => ("failed", e.message()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_urls_follow_the_sdk_order() {
        let u = |s: &str| Url::parse(s).unwrap();
        let got: Vec<(String, bool)> = discovery_urls(&u("https://a.test/")).into_iter().map(|(x, o)| (x.to_string(), o)).collect();
        assert_eq!(got, vec![("https://a.test/.well-known/oauth-authorization-server".into(), true), ("https://a.test/.well-known/openid-configuration".into(), false)]);
        let got: Vec<String> = discovery_urls(&u("https://a.test/tenant/")).into_iter().map(|(x, _)| x.to_string()).collect();
        assert_eq!(
            got,
            vec![
                "https://a.test/.well-known/oauth-authorization-server/tenant",
                "https://a.test/.well-known/openid-configuration/tenant",
                "https://a.test/tenant/.well-known/openid-configuration"
            ]
        );
    }

    #[test]
    fn client_auth_follows_the_registration_then_the_server() {
        let info = |v: Value| v.as_object().unwrap().clone();
        let s = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(client_auth_method(&info(json!({"client_id": "a", "client_secret": "b"})), &[]), "client_secret_basic");
        assert_eq!(client_auth_method(&info(json!({"client_id": "a"})), &[]), "none");
        assert_eq!(client_auth_method(&info(json!({"client_id": "a", "client_secret": "b"})), &s(&["client_secret_post", "none"])), "client_secret_post");
        assert_eq!(client_auth_method(&info(json!({"client_id": "a", "token_endpoint_auth_method": "none"})), &s(&["none"])), "none");
        assert_eq!(client_auth_method(&info(json!({"client_id": "a", "token_endpoint_auth_method": "none"})), &s(&["client_secret_basic"])), "none");
        assert_eq!(client_auth_method(&info(json!({"client_id": "a", "client_secret": "b"})), &s(&["private_key_jwt"])), "client_secret_post");
    }

    #[test]
    fn issuers_match_tolerates_one_trailing_slash() {
        assert!(issuers_match("https://a.test", "https://a.test/"));
        assert!(issuers_match("https://A.test:443/x", "https://a.test/x"));
        assert!(!issuers_match("https://a.test/x", "https://a.test/y"));
        assert!(issuers_match("not a url", "not a url"));
    }

    #[test]
    fn pkce_makes_43_unreserved_characters_and_their_s256() {
        let (v, c) = pkce();
        assert_eq!(v.len(), 43);
        assert!(v.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b)));
        assert_eq!(c, challenge_of(&v));
        // RFC 7636 appendix B
        assert_eq!(challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn an_oauth_error_without_a_description_reads_as_its_class() {
        let e = AuthError::OAuth { kind: Kind::InvalidGrant, name: "InvalidGrantError".into(), message: String::new() };
        assert_eq!(e.message(), "InvalidGrantError");
        let e = AuthError::OAuth { kind: Kind::InvalidGrant, name: "InvalidGrantError".into(), message: "revoked".into() };
        assert_eq!(e.message(), "revoked");
    }

    #[test]
    fn set_param_replaces_in_place_and_appends_when_absent() {
        let mut u = Url::parse("https://a.test/auth?x=1&prompt=login&y=2&prompt=none").unwrap();
        set_param(&mut u, "prompt", "consent");
        set_param(&mut u, "z", "a b");
        assert_eq!(u.query(), Some("x=1&prompt=consent&y=2&z=a+b"));
    }

    #[test]
    fn a_resource_is_allowed_under_its_configured_path_only() {
        let u = |s: &str| Url::parse(s).unwrap();
        assert!(resource_allowed(&u("https://a.test/mcp"), &u("https://a.test/mcp")));
        assert!(resource_allowed(&u("https://a.test/mcp/x"), &u("https://a.test/")));
        assert!(!resource_allowed(&u("https://a.test/mcp2"), &u("https://a.test/mcp")));
        assert!(!resource_allowed(&u("https://b.test/mcp"), &u("https://a.test/mcp")));
    }

    #[test]
    fn the_redirect_encodes_the_id_as_encode_uri_component() {
        assert_eq!(fixed_redirect_url(8787, "x"), "http://127.0.0.1:8787/oauth/callback/x");
        assert_eq!(fixed_redirect_url(8787, "a b/c"), "http://127.0.0.1:8787/oauth/callback/a%20b%2Fc");
    }
}
