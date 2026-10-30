//! `GET /models` (routes/apps.mjs): the model picker's catalog from
//! opencodex's `/v1/models`, its `--fast` twins folded into a `fast` flag,
//! each model's effort ladder and default, whether it reaches an image
//! model, and its route (opencodex's provider, from `~/.opencodex/
//! config.json`: a subscription login or an API key).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use hyper::StatusCode;
use serde_json::{json, Map, Value};

use crate::hub::model::{images, Settings};
use crate::hub::types::{self, HubRequest, HubResponse};
use crate::js::{string_of, truthy};

/// opencodex's providers: id → (its `authMode`, its label), and the default.
#[derive(Clone, Debug)]
struct Conns {
    providers: HashMap<String, (Value, Value)>,
    def: Value,
}

impl Default for Conns {
    fn default() -> Self {
        Self { providers: HashMap::new(), def: json!("openai") }
    }
}

const CONN_TTL: Duration = Duration::from_secs(30);

/// `connCache`, per home dir.
fn conn_cache() -> &'static Mutex<HashMap<PathBuf, (Instant, Conns)>> {
    static C: std::sync::OnceLock<Mutex<HashMap<PathBuf, (Instant, Conns)>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

fn read_conns(home: &Path) -> Option<Conns> {
    let c: Value = serde_json::from_str(&std::fs::read_to_string(home.join(".opencodex").join("config.json")).ok()?).ok()?;
    let mut providers = HashMap::new();
    if let Some(ps) = c.get("providers").filter(|p| truthy(p)) {
        let entries: Vec<(String, &Value)> = match ps {
            Value::Object(m) => m.iter().map(|(k, v)| (k.clone(), v)).collect(),
            Value::Array(a) => a.iter().enumerate().map(|(i, v)| (i.to_string(), v)).collect(),
            _ => Vec::new(),
        };
        for (id, p) in entries {
            if p.is_null() {
                // `p.authMode` on null throws: the file reads as unreadable
                return None;
            }
            let pick = |keys: &[&str]| keys.iter().filter_map(|k| p.get(*k)).find(|v| truthy(v)).cloned().unwrap_or(json!(""));
            providers.insert(id, (pick(&["authMode"]), pick(&["label", "name"])));
        }
    }
    let def = c.get("defaultProvider").filter(|d| truthy(d)).cloned().unwrap_or(json!("openai"));
    Some(Conns { providers, def })
}

/// `opencodexConnections()`: the config file, re-read every 30 s (a
/// failed read keeps the last one for as long).
fn connections(home: &Path) -> Conns {
    let mut cache = conn_cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, conns)) = cache.get(home) {
        if at.elapsed() < CONN_TTL {
            return conns.clone();
        }
    }
    let conns = read_conns(home).unwrap_or_else(|| cache.get(home).map(|(_, c)| c.clone()).unwrap_or_default());
    cache.insert(home.to_owned(), (Instant::now(), conns.clone()));
    conns
}

fn conn_label(conn: &str) -> Option<&'static str> {
    Some(match conn {
        "anthropic-apikey" | "anthropic" => "Anthropic",
        "openai" => "OpenAI",
        "xai" => "xAI",
        "openrouter" => "OpenRouter",
        "google" | "gemini" => "Google",
        "deepseek" => "DeepSeek",
        "moonshot" => "Moonshot",
        "groq" => "Groq",
        "together" => "Together",
        "fireworks" => "Fireworks",
        _ => return None,
    })
}

/// `conn.replace(/-?apikey$/i, "").replace(/^\w/, upper)`.
fn conn_name(conn: &str) -> String {
    let lower = conn.to_ascii_lowercase();
    let mut s = conn;
    if lower.ends_with("apikey") {
        s = &conn[..conn.len() - 6];
        if s.ends_with('-') {
            s = &s[..s.len() - 1];
        }
    }
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() || c == '_' => c.to_ascii_uppercase().to_string() + chars.as_str(),
        _ => s.to_owned(),
    }
}

/// `connectionOf(id, owner, conns)`: the model's provider, its group
/// label, and whether it is a subscription (anything but an API key).
fn connection_of(id: &str, owner: Option<&Value>, conns: &Conns, out: &mut Map<String, Value>) {
    let conn = match id.find('/') {
        Some(slash) if slash > 0 => id[..slash].to_owned(),
        _ => string_of(owner.filter(|o| truthy(o)).unwrap_or(&conns.def)).to_lowercase(),
    };
    let (auth, label) = conns.providers.get(&conn).cloned().unwrap_or((Value::Null, Value::Null));
    let auth = if truthy(&auth) { auth } else { json!(if conn.contains("key") { "key" } else { "forward" }) };
    let group = if truthy(&label) { label } else { json!(conn_label(&conn).map(str::to_owned).unwrap_or_else(|| conn_name(&conn))) };
    out.insert("connection".to_owned(), json!(conn));
    out.insert("group".to_owned(), group);
    out.insert("subscription".to_owned(), json!(auth != json!("key")));
}

/// `String(e.label || e.value).replace(/ Effort$/i, "")`.
fn effort_label(e: &Value) -> String {
    let v = e.get("label").filter(|l| truthy(l)).or_else(|| e.get("value"));
    let s = v.map(string_of).unwrap_or_else(|| "undefined".to_owned());
    if s.len() >= 7 && s.is_char_boundary(s.len() - 7) && s[s.len() - 7..].eq_ignore_ascii_case(" effort") {
        s[..s.len() - 7].to_owned()
    } else {
        s
    }
}

/// The catalog, or `None` where Node's chain throws.
fn fold(j: &Value, draws: &HashMap<String, bool>, conns: &Conns) -> Option<Vec<Value>> {
    let data = j.get("data")?.as_array()?;
    let mut ids = std::collections::HashSet::new();
    for m in data {
        ids.insert(string_of(m.as_object()?.get("id").unwrap_or(&Value::Null)));
    }
    let mut models = Vec::new();
    for m in data {
        let id = m.get("id")?.as_str()?;
        if id.ends_with("--fast") {
            continue;
        }
        let efforts = match m.get("reasoning_efforts").filter(|e| truthy(e)) {
            None => Vec::new(),
            Some(list) => {
                let mut out = Vec::new();
                for e in list.as_array()? {
                    if e.is_null() {
                        return None;
                    }
                    let mut o = Map::new();
                    if let Some(v) = e.get("value") {
                        o.insert("value".to_owned(), v.clone());
                    }
                    o.insert("label".to_owned(), json!(effort_label(e)));
                    out.push(Value::Object(o));
                }
                out
            }
        };
        let mut o = Map::new();
        o.insert("id".to_owned(), json!(id));
        o.insert("owner".to_owned(), m.get("owned_by").filter(|o| truthy(o)).cloned().unwrap_or(json!("")));
        o.insert("fast".to_owned(), json!(ids.contains(&format!("{id}--fast"))));
        o.insert("efforts".to_owned(), Value::Array(efforts));
        o.insert("defaultEffort".to_owned(), m.get("reasoning_effort").filter(|e| truthy(e)).cloned().unwrap_or(json!("")));
        o.insert("image".to_owned(), json!(draws.get(id).copied().unwrap_or(false)));
        connection_of(id, m.get("owned_by"), conns, &mut o);
        models.push(Value::Object(o));
    }
    Some(models)
}

/// The answer's body, for the given home dir.
pub async fn models_json(settings: &Settings, home: &Path) -> Value {
    let unreachable = || json!({ "ok": false, "default": settings.model, "models": [], "error": format!("opencodex at {} unreachable", settings.opencodex) });
    let Ok(j) = images::get_json(&format!("{}/v1/models", settings.opencodex)).await else { return unreachable() };
    let draws = images::image_models(&settings.opencodex).await;
    let conns = connections(home);
    match fold(&j, &draws, &conns) {
        Some(models) => json!({ "ok": true, "default": settings.model, "models": models }),
        None => unreachable(),
    }
}

/// `GET /models`.
pub async fn handle(settings: &Settings, req: &HubRequest) -> HubResponse {
    if req.url != "/models" {
        // Node matches the whole url: with a query, no route answers
        use http_body_util::BodyExt;
        let mut res = hyper::Response::new(http_body_util::Full::new(bytes::Bytes::from_static(b"not found")).map_err(|never| match never {}).boxed());
        *res.status_mut() = StatusCode::NOT_FOUND;
        return res;
    }
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    types::json(StatusCode::OK, &models_json(settings, &home).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_connections_name_the_route_and_its_kind() {
        let mut conns = Conns::default();
        conns.providers.insert("openai".into(), (json!("chatgpt"), json!("ChatGPT")));
        conns.providers.insert("anthropic-apikey".into(), (json!("apikey"), json!("")));
        let of = |id: &str, owner: Value| {
            let mut m = Map::new();
            connection_of(id, Some(&owner), &conns, &mut m);
            Value::Object(m)
        };
        assert_eq!(of("claude", json!("anthropic-apikey")), json!({"connection": "anthropic-apikey", "group": "Anthropic", "subscription": true}));
        assert_eq!(of("plain", json!("")), json!({"connection": "openai", "group": "ChatGPT", "subscription": true}));
        assert_eq!(of("openrouter/qwen", json!("x")), json!({"connection": "openrouter", "group": "OpenRouter", "subscription": true}));
        assert_eq!(of("m", json!("Mistral-APIKEY")), json!({"connection": "mistral-apikey", "group": "Mistral", "subscription": false}));
        assert_eq!(of("m", json!("acme")), json!({"connection": "acme", "group": "Acme", "subscription": true}));
        assert_eq!(effort_label(&json!({"value": "low", "label": "Low Effort"})), "Low");
        assert_eq!(effort_label(&json!({"value": "x high effort"})), "x high");
        assert_eq!(effort_label(&json!({})), "undefined");
    }
}
