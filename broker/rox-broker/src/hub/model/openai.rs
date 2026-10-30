//! The OpenAI-compatible path (`GEN_PROVIDER=openai`): Node's plain
//! `fetch` to `${GEN_BASE_URL}/chat/completions` with
//! `Authorization: Bearer ${OPENROUTER_API_KEY}` (`Bearer undefined` when
//! unset, as the template literal writes it). No retries.

use serde_json::{json, Value};
use url::Url;

use super::Settings;
use crate::helper::http;

/// An answer: its status and body text. `Err` is fetch's rejection
/// (`fetch failed`).
pub async fn completions(settings: &Settings, body: &Value) -> Result<(u16, String), String> {
    let url = Url::parse(&format!("{}/chat/completions", settings.base_url)).map_err(|_| "fetch failed".to_owned())?;
    let key = settings.openrouter_key.clone().unwrap_or_else(|| "undefined".to_owned());
    let req = http::Req::post(url, crate::js::stringify(body).into_bytes()).header("authorization", &format!("Bearer {key}")).header("content-type", "application/json");
    let res = http::fetch(req).await.map_err(|_| "fetch failed".to_owned())?;
    let status = res.status;
    let text = res.text().await.map_err(|_| "terminated".to_owned())?;
    Ok((status, text))
}

/// `r.json()`: the body parsed, or the parse error.
pub fn json_of(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|e| e.to_string())
}

/// `data.choices?.[0]?.<path>`.
pub fn choice<'a>(data: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut v = data.get("choices")?.get(0)?;
    for p in path {
        v = v.get(*p)?;
    }
    Some(v)
}

/// chatReply's `oaiContent`: Anthropic's base64 image blocks as
/// `image_url` parts; a string as is. Anything else is the TypeError
/// `c.map` throws.
pub fn oai_content(c: Option<&Value>) -> Result<Value, String> {
    match c {
        Some(Value::String(s)) => Ok(Value::String(s.clone())),
        Some(Value::Array(blocks)) => Ok(Value::Array(
            blocks
                .iter()
                .map(|b| {
                    if b.get("type").and_then(Value::as_str) == Some("image") {
                        let src = b.get("source").cloned().unwrap_or(Value::Null);
                        let part = |k: &str| src.get(k).map(crate::js::string_of).unwrap_or_else(|| "undefined".to_owned());
                        json!({ "type": "image_url", "image_url": { "url": format!("data:{};base64,{}", part("media_type"), part("data")) } })
                    } else {
                        b.clone()
                    }
                })
                .collect(),
        )),
        None => Err("Cannot read properties of undefined (reading 'map')".to_owned()),
        Some(Value::Null) => Err("Cannot read properties of null (reading 'map')".to_owned()),
        Some(_) => Err("c.map is not a function".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_oai_content_maps_image_blocks() {
        let blocks = json!([{"type": "text", "text": "a"}, {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAA"}}]);
        assert_eq!(oai_content(Some(&blocks)).unwrap(), json!([{"type": "text", "text": "a"}, {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAA"}}]));
        assert_eq!(oai_content(Some(&json!("hi"))).unwrap(), json!("hi"));
        assert_eq!(oai_content(None).unwrap_err(), "Cannot read properties of undefined (reading 'map')");
    }
}
