//! The model client (plan 5.3; model.mjs, the provider settings of
//! config.mjs, the image side of avatars.mjs): Anthropic Messages through
//! opencodex, routed per thread (`choice`), streamed or not, with the SDK's
//! retries, headers and error texts (`anthropic`, `sse`); the
//! OpenAI-compatible path (`openai`); the legacy generator's calls
//! (`ask_model`, `classify_intent`, `chat_reply`, `extract_code`); image
//! generation (`images`); and `GET /models` (`hub/routes/models.rs`).
//!
//! `ModelClient` is the hub's `ModelApi`: its errors read as Node's job
//! details do, `llm-api <status>: <SDK message>` (`anthropic::parse_llm`
//! splits them). The tool loop (5.4) streams through it and folds the
//! events with `sse::collect`, whose callback sees each event and the
//! message so far (the placeholders' `streamEvent`).

pub mod anthropic;
pub mod choice;
pub mod images;
pub mod mock;
pub mod openai;
pub mod prompts;
pub mod sse;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use serde_json::{json, Value};

use self::anthropic::{parse_llm, Endpoint};
use super::ctx::HubCtx;
use super::traits::ModelApi;
use super::types::{BoxFuture, Pattern, TurnCtx, Verb};
use super::Registry;

/// The provider settings (config.mjs), read once from the environment.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// `GEN_PROVIDER`, default `anthropic`
    pub provider: String,
    /// `GEN_BASE_URL`, the OpenAI-compatible endpoint
    pub base_url: String,
    /// `GEN_MODEL`, else the provider's default
    pub model: String,
    /// `Number(GEN_MAX_TOKENS || 20000)` (`null` when not a number)
    pub max_tokens: Value,
    /// `MOCK` set
    pub mock: bool,
    /// `OPENCODEX_URL`
    pub opencodex: String,
    /// the default route: `ANTHROPIC_BASE_URL || OPENCODEX`
    pub direct_base: String,
    /// `ANTHROPIC_API_KEY || "opencodex"`
    pub direct_key: String,
    /// `ANTHROPIC_AUTH_TOKEN`, which the SDK sends as a bearer token
    pub auth_token: Option<String>,
    /// `OPENROUTER_API_KEY`, as the OpenAI path sends it
    pub openrouter_key: Option<String>,
}

/// `Number(s)` for a numeric env value.
fn js_number(s: &str) -> Value {
    let t = s.trim_matches(crate::js::is_space);
    if t.is_empty() {
        return json!(0);
    }
    match t.parse::<f64>() {
        Ok(f) if f.is_finite() && !t.contains(|c: char| c.is_ascii_alphabetic() && c != 'e' && c != 'E') => {
            if f.fract() == 0.0 && f.abs() < 9.007_199_254_740_992e15 {
                json!(f as i64)
            } else {
                json!(f)
            }
        }
        _ => Value::Null,
    }
}

impl Settings {
    /// The settings from `env` (a variable's value, `None` when unset).
    pub fn from(env: impl Fn(&str) -> Option<String>) -> Self {
        let set = |k: &str| env(k).filter(|v| !v.is_empty());
        let provider = set("GEN_PROVIDER").unwrap_or_else(|| "anthropic".to_owned());
        let opencodex = set("OPENCODEX_URL").unwrap_or_else(|| "http://localhost:8080".to_owned());
        let model = set("GEN_MODEL").unwrap_or_else(|| if provider == "anthropic" { "claude-haiku-5-5" } else { "openai/gpt-4o-mini" }.to_owned());
        Self {
            base_url: set("GEN_BASE_URL").unwrap_or_else(|| "https://openrouter.ai/api/v1".to_owned()),
            model,
            max_tokens: set("GEN_MAX_TOKENS").map(|v| js_number(&v)).unwrap_or_else(|| json!(20000)),
            mock: set("MOCK").is_some(),
            direct_base: set("ANTHROPIC_BASE_URL").unwrap_or_else(|| opencodex.clone()),
            direct_key: set("ANTHROPIC_API_KEY").unwrap_or_else(|| "opencodex".to_owned()),
            auth_token: env("ANTHROPIC_AUTH_TOKEN").map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()),
            openrouter_key: env("OPENROUTER_API_KEY"),
            opencodex,
            provider,
        }
    }

    /// This process's settings.
    pub fn global() -> &'static Settings {
        static S: OnceLock<Settings> = OnceLock::new();
        S.get_or_init(|| Settings::from(|k| std::env::var(k).ok()))
    }

    /// `PROVIDER === "anthropic"`: the Messages client exists.
    pub fn anthropic(&self) -> bool {
        self.provider == "anthropic"
    }

    /// The default route's client (`anthropicDirect`).
    pub fn direct(&self) -> Endpoint {
        Endpoint { base: self.direct_base.clone(), key: self.direct_key.clone(), token: self.auth_token.clone() }
    }

    /// The client a chosen model goes through (`opencodex`).
    pub fn opencodex_endpoint(&self) -> Endpoint {
        Endpoint { base: self.opencodex.clone(), key: "opencodex".to_owned(), token: self.auth_token.clone() }
    }
}

// BUG: a 5xx on the first streamed round reads as "streaming unavailable":
// streaming stays off for the process from then on (MODEL-05). Node's
// `agentStream` (tools.mjs): the tool loop turns it off when a stream
// fails before its first event, and never back on.
static AGENT_STREAM: AtomicBool = AtomicBool::new(true);

/// Whether the tool loop streams (`agentStream`).
pub fn agent_stream() -> bool {
    AGENT_STREAM.load(Ordering::SeqCst)
}

/// `setAgentStream(on)`.
pub fn set_agent_stream(on: bool) {
    AGENT_STREAM.store(on, Ordering::SeqCst);
}

/// The hub's `ModelApi`: Messages routed by the turn's model choice.
pub struct ModelClient {
    settings: Arc<Settings>,
}

impl ModelClient {
    pub fn new(settings: Arc<Settings>) -> Self {
        Self { settings }
    }
}

impl ModelApi for ModelClient {
    fn create(&self, turn: &TurnCtx, body: Value) -> BoxFuture<'static, Result<Value, String>> {
        let (ep, body) = choice::routed(&self.settings, turn.model.as_ref(), body);
        Box::pin(async move { anthropic::create(&ep, &body).await.map_err(|e| e.llm()) })
    }

    fn stream(&self, turn: &TurnCtx, body: Value) -> BoxFuture<'static, Result<tokio::sync::mpsc::Receiver<Result<Value, String>>, String>> {
        let (ep, body) = choice::routed(&self.settings, turn.model.as_ref(), body);
        Box::pin(async move { anthropic::stream(&ep, &body).await.map_err(|e| e.llm()) })
    }
}

/// The text blocks of a message, joined (`content.filter(text).map(text).join("")`).
pub fn text_of(msg: &Value) -> String {
    msg.get("content")
        .and_then(Value::as_array)
        .map(|c| c.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("text")).map(|b| b.get("text").filter(|t| !t.is_null()).map(crate::js::string_of).unwrap_or_default()).collect())
        .unwrap_or_default()
}

fn trim(s: &str) -> &str {
    s.trim_matches(crate::js::is_space)
}

/// A generated module: the model's caption and the TSX.
#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub caption: String,
    pub code: String,
}

pub const NO_MODULE: &str = "model did not return a module";
pub const TRUNCATED: &str = "module truncated at the token limit";

/// The fence `/```(?:tsx?|jsx?)?\s*\n([\s\S]*?)```/` matches first: its
/// start, its end, and the group.
fn fence(text: &str) -> Option<(usize, usize, &str)> {
    let mut from = 0;
    while let Some(p) = text[from..].find("```") {
        let at = from + p;
        let after = at + 3;
        for lang in ["tsx", "ts", "jsx", "js", ""] {
            if !text[after..].starts_with(lang) {
                continue;
            }
            let rest = &text[after + lang.len()..];
            let ws: usize = rest.chars().take_while(|c| crate::js::is_space(*c)).map(char::len_utf8).sum();
            let Some(nl) = rest[..ws].rfind('\n') else { continue };
            let start = after + lang.len() + nl + 1;
            if let Some(close) = text[start..].find("```") {
                return Some((at, start + close + 3, &text[start..start + close]));
            }
        }
        from = at + 1;
    }
    None
}

/// `/```[\s\S]*?```/g` replaced by nothing.
fn drop_fences(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(open) = rest.find("```") {
        let Some(close) = rest[open + 3..].find("```") else { break };
        out.push_str(&rest[..open]);
        rest = &rest[open + 3 + close + 3..];
    }
    out.push_str(rest);
    out
}

/// `/function\s+R\s*\(/`.
fn has_function_r(code: &str) -> bool {
    code.match_indices("function").any(|(i, _)| {
        let rest = &code[i + 8..];
        let ws = rest.len() - rest.trim_start_matches(crate::js::is_space).len();
        if ws == 0 {
            return false;
        }
        let rest = &rest[ws..];
        rest.strip_prefix('R').is_some_and(|r| r.trim_start_matches(crate::js::is_space).starts_with('('))
    })
}

/// `extractCode(raw)`: the first fenced block as the module, the text
/// around it as its caption; a bare `function R(` gets its default export.
pub fn extract_code(raw: &str) -> Result<Module, String> {
    let text = trim(raw);
    let (mut code, mut caption) = (text.to_owned(), String::new());
    if let Some((start, end, group)) = fence(text) {
        code = trim(group).to_owned();
        caption = trim(&drop_fences(&format!("{}{}", &text[..start], &text[end..]))).to_owned();
    }
    if !code.contains("export default") && has_function_r(&code) {
        code.push_str("\nexport default R;\n");
    }
    if !code.contains("export default") || !code.contains('<') {
        return Err(NO_MODULE.to_owned());
    }
    Ok(Module { caption, code })
}

/// What the generator asks (`askModel(job, prompt, prevError, prevCode, name)`).
#[derive(Clone, Debug, Default)]
pub struct Ask<'a> {
    pub prompt: &'a str,
    /// the last try's failure (a compiler error, truncation, no module)
    pub prev_error: Option<&'a str>,
    /// the last try's code, when there was one
    pub prev_code: Option<&'a str>,
    /// the module's name (its `/state` key)
    pub name: &'a str,
    /// whether a Linear key is set (`getLinearKey()`)
    pub linear: bool,
}

/// `askModel`: one module from the model, streamed (`on_text` runs as
/// text streams: the job's "Writing some code"). `MOCK` answers with
/// `mock_module(prompt)`. A failed call is `llm-api …`; a truncated or
/// moduleless reply is its own error, which the generator retries.
pub async fn ask_model(settings: &Settings, api: &dyn ModelApi, turn: &TurnCtx, ask: &Ask<'_>, mock_module: impl FnOnce(&str) -> String, mut on_text: impl FnMut() + Send) -> Result<Module, String> {
    if settings.mock {
        return Ok(Module { caption: String::new(), code: mock_module(ask.prompt) });
    }
    let mut messages = vec![json!({ "role": "user", "content": ask.prompt })];
    if let Some(err) = ask.prev_error.filter(|e| !e.is_empty()) {
        if let Some(code) = ask.prev_code {
            messages.push(json!({ "role": "assistant", "content": code }));
        }
        messages.push(json!({ "role": "user", "content": prompts::follow_up(err) }));
    }
    let system = prompts::system_for(ask.name, ask.linear);
    if settings.anthropic() {
        // haiku-5.5 deprecates temperature — omit it
        let body = json!({ "model": settings.model, "max_tokens": settings.max_tokens, "system": system, "messages": messages });
        let rx = api.stream(turn, body).await?;
        let msg = sse::collect(rx, |event, acc| {
            let last_is_text = acc.snapshot().and_then(|s| s.get("content")).and_then(Value::as_array).and_then(|c| c.last()).and_then(|b| b.get("type")).and_then(Value::as_str) == Some("text");
            if event.get("type").and_then(Value::as_str) == Some("content_block_delta") && event.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta") && last_is_text {
                on_text();
            }
        })
        .await?;
        if msg.get("stop_reason").and_then(Value::as_str) == Some("max_tokens") {
            return Err(TRUNCATED.to_owned());
        }
        return extract_code(&text_of(&msg));
    }
    let mut all = vec![json!({ "role": "system", "content": system })];
    all.extend(messages);
    let body = json!({ "model": settings.model, "messages": all, "temperature": 0.4, "max_tokens": settings.max_tokens });
    let (status, text) = openai::completions(settings, &body).await?;
    if !(200..300).contains(&status) {
        return Err(anthropic::llm(Some(status), &text));
    }
    let data = openai::json_of(&text)?;
    if openai::choice(&data, &["finish_reason"]).and_then(Value::as_str) == Some("length") {
        return Err(TRUNCATED.to_owned());
    }
    let content = openai::choice(&data, &["message", "content"]).filter(|c| crate::js::truthy(c)).map(crate::js::string_of).unwrap_or_default();
    extract_code(&content)
}

/// `classifyIntent(prompt)`: `"ui"` when the user wants something built,
/// else `"chat"`; `"ui"` when the classifier fails (or `MOCK`).
pub async fn classify_intent(settings: &Settings, api: &dyn ModelApi, turn: &TurnCtx, prompt: &str) -> &'static str {
    if settings.mock {
        return mock::INTENT;
    }
    let answer: Result<String, String> = async {
        if settings.anthropic() {
            let body = json!({ "model": settings.model, "max_tokens": 8, "system": prompts::CLASSIFY, "messages": [{ "role": "user", "content": prompt }] });
            let m = api.create(turn, body).await.map_err(|e| parse_llm(&e).1.to_owned())?;
            return Ok(text_of(&m));
        }
        let body = json!({ "model": settings.model, "messages": [{ "role": "system", "content": prompts::CLASSIFY }, { "role": "user", "content": prompt }], "max_tokens": 8 });
        let (_, text) = openai::completions(settings, &body).await?;
        let data = openai::json_of(&text)?;
        match openai::choice(&data, &["message", "content"]).filter(|c| crate::js::truthy(c)) {
            None => Ok(String::new()),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(_) => Err("answer.trim is not a function".to_owned()),
        }
    }
    .await;
    match answer {
        Ok(a) if trim(&a).to_uppercase().starts_with("UI") => "ui",
        Ok(_) => "chat",
        Err(e) => {
            eprintln!("  intent classify failed: {e} — generating anyway");
            "ui"
        }
    }
}

/// `chatReply(prompt, thread, job)`: a conversational answer with the
/// thread's last ten turns (notices left out) as context. `history` is the
/// thread's turns as `/turns` shows them; `content` the user message as
/// the tool loop's `userContent` builds it.
pub async fn chat_reply(settings: &Settings, api: &dyn ModelApi, turn: &TurnCtx, history: &[Value], content: Value) -> String {
    let kept: Vec<&Value> = history.iter().filter(|t| t.get("role").and_then(Value::as_str) != Some("notice")).collect();
    let mut messages: Vec<Value> = kept[kept.len().saturating_sub(10)..]
        .iter()
        .map(|t| {
            if t.get("role").and_then(Value::as_str) == Some("user") {
                let mut m = serde_json::Map::new();
                m.insert("role".to_owned(), json!("user"));
                if let Some(text) = t.get("text") {
                    m.insert("content".to_owned(), text.clone());
                }
                Value::Object(m)
            } else {
                let content = match (t.get("name").filter(|n| crate::js::truthy(n)), t.get("text").filter(|x| crate::js::truthy(x))) {
                    (Some(name), _) => json!(format!("[built widget {}]", crate::js::string_of(name))),
                    (None, Some(text)) => text.clone(),
                    (None, None) => json!(""),
                };
                json!({ "role": "assistant", "content": content })
            }
        })
        .collect();
    messages.push(json!({ "role": "user", "content": content }));
    if settings.mock {
        return mock::CHAT_REPLY.to_owned();
    }
    let reply: Result<String, String> = async {
        if settings.anthropic() {
            let body = json!({ "model": settings.model, "max_tokens": 1024, "system": prompts::CHAT, "messages": messages });
            let m = api.create(turn, body).await.map_err(|e| parse_llm(&e).1.to_owned())?;
            return Ok(trim(&text_of(&m)).to_owned());
        }
        // BUG: chatReply's system prompt never reaches the OpenAI path (only
        // the Anthropic call passes `system`) (MODEL-04)
        let mut oai = Vec::new();
        for m in &messages {
            let mut m = m.as_object().cloned().unwrap_or_default();
            let c = openai::oai_content(m.get("content"))?;
            m.insert("content".to_owned(), c);
            oai.push(Value::Object(m));
        }
        let body = json!({ "model": settings.model, "messages": oai, "max_tokens": 1024 });
        let (_, text) = openai::completions(settings, &body).await?;
        let data = openai::json_of(&text)?;
        match openai::choice(&data, &["message", "content"]).filter(|c| crate::js::truthy(c)) {
            None => Ok("(empty reply)".to_owned()),
            Some(Value::String(s)) => Ok(trim(s).to_owned()),
            Some(_) => Err("(intermediate value).trim is not a function".to_owned()),
        }
    }
    .await;
    reply.unwrap_or_else(|e| format!("_(model error — {})_", crate::js::slice_utf16(if e.is_empty() { "Error" } else { &e }, 200)))
}

/// Puts the model client in the hub and answers `GET /models`.
pub fn install(routes: &mut Registry, ctx: &mut HubCtx) {
    let settings = Arc::new(Settings::global().clone());
    ctx.model = Some(Arc::new(ModelClient::new(settings.clone())));
    // /avatar/image/gen draws through the image models (5.2's seam)
    ctx.ext.insert(super::routes::bots::AvatarImage(Arc::new(|ctx: Arc<HubCtx>, description: String, out: std::path::PathBuf| -> super::types::BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            let api = ctx.model.clone().ok_or_else(|| "no model client".to_owned())?;
            let local = crate::helper::local::tools(&ctx);
            images::generate_avatar_image(Settings::global(), api.as_ref(), &super::types::TurnCtx::default(), local.as_ref(), &description, &out).await.map(|_| ())
        })
    })));
    routes.route(Verb::Get, Pattern::Exact("/models"), move |_, req| {
        let settings = settings.clone();
        async move { super::routes::models::handle(&settings, &req).await }
    });
}

#[cfg(test)]
pub(crate) mod testkit;

#[cfg(test)]
mod tests;
