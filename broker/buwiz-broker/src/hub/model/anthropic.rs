//! Anthropic Messages over HTTP as the SDK (`@anthropic-ai/sdk` 0.133)
//! sends it for Node: `POST <base>/v1/messages` with `anthropic-version`,
//! `x-api-key` (and `Authorization` when `ANTHROPIC_AUTH_TOKEN` is set), the
//! body as `JSON.stringify` writes it; 2 retries on 408, 409, 429, 5xx and
//! connection errors (`x-should-retry` obeyed; `retry-after-ms`, then
//! `retry-after` seconds, else 0.5 s doubling with up to 25% jitter); a
//! 10-minute timeout; the SDK's error messages. Plain hyper and rustls
//! (`helper/http.rs`): no Keychain.

use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc;
use url::Url;

use super::sse::{self, Item, LineDecoder, SseDecoder};
use crate::helper::http;

pub const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_RETRIES: u32 = 2;
/// The SDK's default timeout (10 minutes), the non-streaming budget too.
const TIMEOUT: Duration = Duration::from_secs(600);

/// Where a call goes: a base URL and its key.
#[derive(Clone, Debug, PartialEq)]
pub struct Endpoint {
    pub base: String,
    pub key: String,
    /// `ANTHROPIC_AUTH_TOKEN`, which the SDK reads for every client
    pub token: Option<String>,
}

/// An API failure: the HTTP status when there was one, and the SDK's
/// message (`500 {"type":"error",…}`, `Connection error.`, …).
#[derive(Clone, Debug, PartialEq)]
pub struct ApiError {
    pub status: Option<u16>,
    pub message: String,
}

impl ApiError {
    fn new(status: Option<u16>, message: impl Into<String>) -> Self {
        Self { status, message: message.into() }
    }

    /// Node's `llm-api ${e.status ?? ""}: ${e.message}`.
    pub fn llm(&self) -> String {
        llm(self.status, &self.message)
    }
}

pub fn llm(status: Option<u16>, message: &str) -> String {
    format!("llm-api {}: {message}", status.map(|s| s.to_string()).unwrap_or_default())
}

/// The parts of an `llm-api <status>: <message>` text (as `ModelApi`
/// errors read): the status, and the SDK's message. Any other text is a
/// message with no status.
pub fn parse_llm(text: &str) -> (Option<u16>, &str) {
    if let Some(rest) = text.strip_prefix("llm-api ") {
        if let Some((status, message)) = rest.split_once(": ") {
            if status.is_empty() {
                return (None, message);
            }
            if let Ok(s) = status.parse() {
                return (Some(s), message);
            }
        }
    }
    (None, text)
}

/// `APIError.makeMessage(status, error, message)`.
pub fn error_message(status: Option<u16>, error: Option<&Value>, message: Option<&str>) -> String {
    let msg = match error {
        Some(e) if crate::js::truthy(e) => match e.get("message").filter(|m| crate::js::truthy(m)) {
            Some(Value::String(m)) => Some(m.clone()),
            Some(m) => Some(crate::js::stringify(m)),
            None => Some(crate::js::stringify(e)),
        },
        _ => message.map(str::to_owned),
    }
    .filter(|m| !m.is_empty());
    match (status, msg) {
        (Some(s), Some(m)) => format!("{s} {m}"),
        (Some(s), None) => format!("{s} status code (no body)"),
        (None, Some(m)) => m,
        (None, None) => "(no status code or body)".to_owned(),
    }
}

/// The error a non-2xx answer becomes: its body as JSON when it parses,
/// else as text.
fn status_error(status: u16, text: &str) -> ApiError {
    match serde_json::from_str::<Value>(text) {
        Ok(json) if crate::js::truthy(&json) => ApiError::new(Some(status), error_message(Some(status), Some(&json), None)),
        _ => ApiError::new(Some(status), error_message(Some(status), None, Some(text))),
    }
}

/// `baseURL + path`, with one `/` between them.
pub fn url_of(base: &str, path: &str) -> String {
    if base.ends_with('/') && path.starts_with('/') {
        format!("{base}{}", &path[1..])
    } else {
        format!("{base}{path}")
    }
}

/// The SDK's refusal of a non-streaming call that may outlast its
/// 10 minutes (`calculateNonstreamingTimeout`): `max_tokens` above
/// 128000 / 6, or above the model's own non-streaming cap.
pub fn nonstreaming_refusal(body: &Value) -> Option<String> {
    let max = body.get("max_tokens").and_then(Value::as_f64)?;
    let cap = match body.get("model").and_then(Value::as_str) {
        Some("claude-opus-4@20250514" | "anthropic.claude-opus-4-1-20250805-v1:0" | "claude-opus-4-1@20250805") => Some(8192.0),
        _ => None,
    };
    let expected = 60.0 * 60.0 * 1000.0 * max / 128000.0;
    (expected > 600000.0 || cap.is_some_and(|c| max > c))
        .then(|| "Streaming is required for operations that may take longer than 10 minutes. See https://github.com/anthropics/anthropic-sdk-typescript#long-requests for more details".to_owned())
}

/// `parseFloat`: the longest number at the start, `NaN` as `None`.
fn parse_float(s: &str) -> Option<f64> {
    let s = s.trim_start_matches(crate::js::is_space);
    let b = s.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    if s[i..].starts_with("Infinity") {
        return Some(if b.first() == Some(&b'-') { f64::NEG_INFINITY } else { f64::INFINITY });
    }
    let digits = |i: &mut usize| {
        let s = *i;
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i - s
    };
    let mut n = digits(&mut i);
    if b.get(i) == Some(&b'.') {
        i += 1;
        n += digits(&mut i);
    }
    if n == 0 {
        return None;
    }
    let mantissa_end = i;
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if digits(&mut j) > 0 {
            i = j;
        }
    }
    s[..i.max(mantissa_end)].parse().ok()
}

/// How long to wait before retry number `max - remaining + 1`.
fn retry_delay(headers: Option<&hyper::HeaderMap>, remaining: u32) -> Duration {
    let get = |name: &str| headers.and_then(|h| h.get(name)).and_then(|v| v.to_str().ok());
    let mut ms = get("retry-after-ms").and_then(parse_float);
    if let Some(after) = get("retry-after").filter(|_| !ms.is_some_and(|m| m != 0.0)) {
        // an HTTP date is not read here: it falls back to the default
        ms = parse_float(after).map(|s| s * 1000.0);
    }
    let ms = match ms {
        Some(m) if m > 0.0 && m <= 2f64.powi(31) - 1.0 => m,
        _ => {
            let n = MAX_RETRIES - remaining;
            let seconds = (0.5 * 2f64.powi(n as i32)).min(8.0);
            seconds * (1.0 - jitter() * 0.25) * 1000.0
        }
    };
    Duration::from_micros((ms * 1000.0) as u64)
}

/// A number in [0, 1) (`Math.random()` for the jitter).
fn jitter() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    (h.finish() >> 11) as f64 / (1u64 << 53) as f64
}

fn should_retry(res: &http::Resp) -> bool {
    match res.header("x-should-retry").as_deref() {
        Some("true") => return true,
        Some("false") => return false,
        _ => {}
    }
    matches!(res.status, 408 | 409 | 429) || res.status >= 500
}

fn request(ep: &Endpoint, body: &[u8]) -> Result<http::Req, ApiError> {
    let url = Url::parse(&url_of(&ep.base, "/v1/messages")).map_err(|e| ApiError::new(None, format!("Invalid URL: {e}")))?;
    let mut req = http::Req::post(url, body.to_vec()).header("accept", "application/json").header("anthropic-version", ANTHROPIC_VERSION).header("x-api-key", &ep.key);
    if let Some(token) = &ep.token {
        req = req.header("authorization", &format!("Bearer {token}"));
    }
    Ok(req.header("content-type", "application/json"))
}

/// `POST /v1/messages` until an answer that is not retried: the 2xx
/// response, or the error.
pub async fn post(ep: &Endpoint, body: &Value) -> Result<http::Resp, ApiError> {
    let bytes = crate::js::stringify(body).into_bytes();
    let req = request(ep, &bytes)?;
    let mut remaining = MAX_RETRIES;
    loop {
        match tokio::time::timeout(TIMEOUT, http::send(&req)).await {
            Ok(Ok(res)) if res.ok() => return Ok(res),
            Ok(Ok(res)) => {
                if remaining > 0 && should_retry(&res) {
                    let wait = retry_delay(Some(&res.headers), remaining);
                    res.cancel();
                    tokio::time::sleep(wait).await;
                    remaining -= 1;
                    continue;
                }
                let status = res.status;
                let text = res.text().await.unwrap_or_else(|e| e);
                return Err(status_error(status, &text));
            }
            failed => {
                let timed_out = match &failed {
                    Err(_) => true,
                    Ok(Err(e)) => {
                        let e = e.to_ascii_lowercase();
                        e.contains("timed out") || e.contains("timeout") || e.contains("time out")
                    }
                    Ok(Ok(_)) => false,
                };
                if remaining > 0 {
                    tokio::time::sleep(retry_delay(None, remaining)).await;
                    remaining -= 1;
                    continue;
                }
                return Err(ApiError::new(None, if timed_out { "Request timed out." } else { "Connection error." }));
            }
        }
    }
}

/// `messages.create(body)`, not streamed: the message.
pub async fn create(ep: &Endpoint, body: &Value) -> Result<Value, ApiError> {
    if !body.get("stream").is_some_and(crate::js::truthy) {
        if let Some(refusal) = nonstreaming_refusal(body) {
            return Err(ApiError::new(None, refusal));
        }
    }
    let res = post(ep, body).await?;
    let text = res.text().await.map_err(|_| ApiError::new(None, "terminated"))?;
    serde_json::from_str(&text).map_err(|e| ApiError::new(None, e.to_string()))
}

/// `messages.create({...body, stream: true})`: the events as they come,
/// each an `Err` in the `llm-api` form once the stream fails.
pub async fn stream(ep: &Endpoint, body: &Value) -> Result<mpsc::Receiver<Result<Value, String>>, ApiError> {
    let mut body = body.clone();
    if let Some(b) = body.as_object_mut() {
        b.insert("stream".to_owned(), Value::Bool(true));
    }
    let mut res = post(ep, &body).await?;
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        let mut lines = LineDecoder::default();
        let mut events = SseDecoder::default();
        let send_line = |line: &str, events: &mut SseDecoder| -> Option<Result<Value, String>> {
            let sse = events.decode(line)?;
            match sse::item(&sse) {
                Ok(Item::Event(v)) => Some(Ok(v)),
                Ok(Item::Skip) => None,
                Ok(Item::Error(m)) => Some(Err(llm(None, &m))),
                Err(m) => Some(Err(llm(None, &m))),
            }
        };
        loop {
            let chunk = match res.chunk().await {
                Ok(Some(c)) => c,
                Ok(None) => break,
                Err(_) => {
                    let _ = tx.send(Err(llm(None, "terminated"))).await;
                    return;
                }
            };
            for line in lines.decode(&chunk) {
                if let Some(item) = send_line(&line, &mut events) {
                    let failed = item.is_err();
                    if tx.send(item).await.is_err() || failed {
                        return;
                    }
                }
            }
        }
        for line in lines.flush().into_iter().chain([String::new()]) {
            if let Some(item) = send_line(&line, &mut events) {
                let failed = item.is_err();
                if tx.send(item).await.is_err() || failed {
                    return;
                }
            }
        }
    });
    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn model_error_messages_read_as_the_sdks() {
        let body = json!({"type": "error", "error": {"type": "api_error", "message": "upstream exploded"}});
        assert_eq!(status_error(500, &crate::js::stringify(&body)).llm(), r#"llm-api 500: 500 {"type":"error","error":{"type":"api_error","message":"upstream exploded"}}"#);
        assert_eq!(status_error(400, r#"{"message":"bad"}"#).message, "400 bad");
        assert_eq!(status_error(400, r#"{"message":{"a":1}}"#).message, r#"400 {"a":1}"#);
        assert_eq!(status_error(502, "Bad Gateway").message, "502 Bad Gateway");
        assert_eq!(status_error(502, "").message, "502 status code (no body)");
        assert_eq!(status_error(502, "null").message, "502 null");
        assert_eq!(ApiError::new(None, "Connection error.").llm(), "llm-api : Connection error.");
        assert_eq!(parse_llm("llm-api 500: 500 x: y"), (Some(500), "500 x: y"));
        assert_eq!(parse_llm("llm-api : terminated"), (None, "terminated"));
        assert_eq!(parse_llm("fetch failed"), (None, "fetch failed"));
    }

    #[test]
    fn model_nonstreaming_refuses_above_the_sdks_budget() {
        assert_eq!(nonstreaming_refusal(&json!({"model": "m", "max_tokens": 20000})), None);
        assert!(nonstreaming_refusal(&json!({"model": "m", "max_tokens": 21334})).unwrap().starts_with("Streaming is required for operations that may take longer than 10 minutes."));
        assert!(nonstreaming_refusal(&json!({"model": "claude-opus-4-1@20250805", "max_tokens": 8193})).is_some());
    }

    #[test]
    fn model_retry_delays_follow_the_headers_then_back_off() {
        let mut h = hyper::HeaderMap::new();
        h.insert("retry-after-ms", "1".parse().unwrap());
        assert_eq!(retry_delay(Some(&h), 2), Duration::from_millis(1));
        let mut h = hyper::HeaderMap::new();
        h.insert("retry-after", "2".parse().unwrap());
        assert_eq!(retry_delay(Some(&h), 2), Duration::from_secs(2));
        let first = retry_delay(None, 2);
        assert!(first > Duration::from_millis(374) && first <= Duration::from_millis(500), "{first:?}");
        let second = retry_delay(None, 1);
        assert!(second > Duration::from_millis(749) && second <= Duration::from_millis(1000), "{second:?}");
        assert_eq!(parse_float("1.5x"), Some(1.5));
        assert_eq!(parse_float("x"), None);
        assert_eq!(url_of("http://a/direct/", "/v1/messages"), "http://a/direct/v1/messages");
    }
}
