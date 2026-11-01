//! Images (the image side of avatars.mjs): which chat models may draw
//! (`imageModels`, `imageCapable`, from opencodex's `/v1/models`, a minute's
//! cache), `generateImage` (opencodex's images relay, then Gemini, then
//! OpenAI with keys from `keyOf`, then an SVG the chat model draws,
//! rasterized by QuickLook) and `editImage`. Commands (`sips`, `qlmanage`,
//! `mv`, `secret get`) run through the hub's `LocalTools`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};
use url::Url;

use super::anthropic::parse_llm;
use super::Settings;
use crate::helper::http;
use crate::hub::traits::{LocalCommand, LocalTools, ModelApi};
use crate::hub::types::TurnCtx;

const GEMINI: &str = "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-image:generateContent";
const OPENAI_IMAGES: &str = "https://api.openai.com/v1/images/generations";
const CACHE: Duration = Duration::from_secs(60);

/// A model id → whether it draws, per opencodex URL (Node's `drawers`,
/// one per process; keyed here so tests on their own fakes stay apart).
type Drawers = HashMap<String, (Option<Instant>, HashMap<String, bool>)>;

fn drawers() -> &'static Mutex<Drawers> {
    static D: std::sync::OnceLock<Mutex<Drawers>> = std::sync::OnceLock::new();
    D.get_or_init(Default::default)
}

/// `GET <url>` and its body as JSON (`(await fetch(url)).json()`).
pub async fn get_json(url: &str) -> Result<Value, String> {
    let url = Url::parse(url).map_err(|_| "fetch failed".to_owned())?;
    let res = http::fetch(http::Req::get(url)).await.map_err(|_| "fetch failed".to_owned())?;
    let text = res.text().await.map_err(|_| "terminated".to_owned())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// `POST <url>` with a JSON body: the status and the body text.
async fn post_json(url: &str, headers: &[(&str, &str)], body: &Value) -> Result<(u16, String), String> {
    let url = Url::parse(url).map_err(|_| "fetch failed".to_owned())?;
    let mut req = http::Req::post(url, crate::js::stringify(body).into_bytes());
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let res = http::fetch(req).await.map_err(|_| "fetch failed".to_owned())?;
    let status = res.status;
    Ok((status, res.text().await.map_err(|_| "terminated".to_owned())?))
}

/// The ids in a `/v1/models` answer and whether each draws: an `image`
/// output modality, or opencodex's OpenAI login (its images relay).
fn draw_map(j: &Value) -> Option<HashMap<String, bool>> {
    let mut ids = HashMap::new();
    for m in j.get("data")?.as_array()? {
        let id = m.get("id")?.as_str()?;
        let image =
            m.pointer("/capabilities/output_modalities").and_then(Value::as_array).is_some_and(|a| a.iter().any(|x| x == "image")) || m.get("owned_by").and_then(Value::as_str) == Some("openai");
        ids.insert(id.strip_suffix("--fast").unwrap_or(id).to_owned(), image);
    }
    Some(ids)
}

/// `imageModels()`: opencodex's models that may draw, for a minute; a
/// failed fetch keeps what was known and tries again next time.
pub async fn image_models(opencodex: &str) -> HashMap<String, bool> {
    {
        let d = drawers().lock().unwrap_or_else(|e| e.into_inner());
        if let Some((Some(at), ids)) = d.get(opencodex) {
            if at.elapsed() < CACHE {
                return ids.clone();
            }
        }
    }
    let fresh = get_json(&format!("{opencodex}/v1/models")).await.ok().and_then(|j| draw_map(&j));
    let mut d = drawers().lock().unwrap_or_else(|e| e.into_inner());
    let entry = d.entry(opencodex.to_owned()).or_insert((None, HashMap::new()));
    if let Some(ids) = fresh {
        *entry = (Some(Instant::now()), ids);
    }
    entry.1.clone()
}

/// `imageCapable(model)`: whether a chat model (its `--fast` twin too) draws.
pub async fn image_capable(opencodex: &str, model: &str) -> bool {
    image_models(opencodex).await.get(model.strip_suffix("--fast").unwrap_or(model)).copied().unwrap_or(false)
}

fn keys() -> &'static Mutex<HashMap<String, String>> {
    static K: std::sync::OnceLock<Mutex<HashMap<String, String>>> = std::sync::OnceLock::new();
    K.get_or_init(Default::default)
}

/// `keyOf(name)`: the environment's value, else `secret get <name>` (kept
/// once found), else "".
pub async fn key_of(local: &dyn LocalTools, name: &str) -> String {
    if let Some(v) = std::env::var(name).ok().filter(|v| !v.is_empty()) {
        return v;
    }
    if let Some(v) = keys().lock().unwrap_or_else(|e| e.into_inner()).get(name) {
        return v.clone();
    }
    let out = local.run(LocalCommand { program: "secret".to_owned(), args: vec!["get".to_owned(), name.to_owned()], ..Default::default() }).await;
    match out {
        Ok(o) if o.code == Some(0) => {
            let v = String::from_utf8_lossy(&o.stdout).trim_matches(crate::js::is_space).to_owned();
            keys().lock().unwrap_or_else(|e| e.into_inner()).insert(name.to_owned(), v.clone());
            v
        }
        _ => String::new(),
    }
}

/// `execFileSync(program, args)`: `Err` with Node's "Command failed: …".
async fn run(local: &dyn LocalTools, program: &str, args: &[&str]) -> Result<(), String> {
    let cmd = LocalCommand { program: program.to_owned(), args: args.iter().map(|a| a.to_string()).collect(), ..Default::default() };
    match local.run(cmd).await {
        Ok(o) if o.code == Some(0) => Ok(()),
        _ => Err(format!("Command failed: {program} {}", args.join(" "))),
    }
}

/// `Buffer.from(s, "base64")`: lenient, as Node decodes.
fn b64(s: &str) -> Vec<u8> {
    let clean: String = s
        .chars()
        .take_while(|c| *c != '=')
        .filter_map(|c| match c {
            '-' => Some('+'),
            '_' => Some('/'),
            c if c.is_ascii_alphanumeric() || c == '+' || c == '/' => Some(c),
            _ => None,
        })
        .collect();
    let usable = clean.len() - if clean.len() % 4 == 1 { 1 } else { 0 };
    base64::engine::general_purpose::STANDARD_NO_PAD.decode(&clean[..usable]).unwrap_or_default()
}

fn path_plus(p: &Path, suffix: &str) -> String {
    format!("{}{suffix}", p.display())
}

/// The raw image written beside `out`, converted to PNG by sips.
async fn save_png(local: &dyn LocalTools, out: &Path, data: &str) -> Result<(), String> {
    let raw = path_plus(out, ".raw");
    std::fs::write(&raw, b64(data)).map_err(|e| e.to_string())?;
    run(local, "sips", &["-s", "format", "png", &raw, "--out", &out.display().to_string()]).await?;
    std::fs::remove_file(&raw).map_err(|e| e.to_string())
}

/// `${j?.error?.message || fallback}`.
fn error_or(j: Option<&Value>, fallback: String) -> String {
    j.and_then(|j| j.pointer("/error/message")).filter(|m| crate::js::truthy(m)).map(crate::js::string_of).unwrap_or(fallback)
}

/// `editImage(src, prompt, out, {size})`: gpt-image on the ChatGPT login,
/// through opencodex's relay. → `"chatgpt"`.
pub async fn edit_image(settings: &Settings, local: &dyn LocalTools, src: &Path, prompt: &str, out: &Path, size: &str) -> Result<String, String> {
    let png = std::fs::read(src).map_err(|e| e.to_string())?;
    let body = json!({
        "prompt": prompt,
        "size": size,
        "images": [{ "image_url": format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)) }],
    });
    let (status, text) = post_json(&format!("{}/v1/images/edits", settings.opencodex), &[("content-type", "application/json")], &body).await?;
    let j = serde_json::from_str::<Value>(&text).ok();
    let Some(data) = j.as_ref().and_then(|j| j.pointer("/data/0/b64_json")).filter(|d| crate::js::truthy(d)).map(crate::js::string_of) else {
        return Err(format!("image edit failed: {}", error_or(j.as_ref(), status.to_string())));
    };
    save_png(local, out, &data).await?;
    Ok("chatgpt".to_owned())
}

/// `generateImage`'s options.
#[derive(Clone, Debug)]
pub struct ImageOpts {
    pub svg_hint: Option<String>,
    pub aspect: String,
    pub size: String,
    pub background: Option<String>,
}

impl Default for ImageOpts {
    fn default() -> Self {
        Self { svg_hint: None, aspect: "1:1".to_owned(), size: "1024x1024".to_owned(), background: None }
    }
}

/// `generateImage(prompt, out, opts)`: the first backend that draws it.
/// → `"chatgpt"`, `"gemini"`, `"openai"` or `"claude-svg"`.
pub async fn generate_image(settings: &Settings, api: &dyn ModelApi, turn: &TurnCtx, local: &dyn LocalTools, prompt: &str, out: &Path, opts: &ImageOpts) -> Result<String, String> {
    let mut errs: Vec<String> = Vec::new();
    // opencodex's images relay (gpt-image on the ChatGPT login)
    let mut body = json!({ "prompt": prompt, "size": opts.size, "n": 1 });
    if let Some(bg) = opts.background.as_ref().filter(|b| !b.is_empty()) {
        body["background"] = json!(bg);
    }
    let relay: Result<Option<String>, String> = async {
        let (status, text) = post_json(&format!("{}/v1/images/generations", settings.opencodex), &[("content-type", "application/json")], &body).await?;
        let j: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        match j.pointer("/data/0/b64_json").filter(|d| crate::js::truthy(d)).map(crate::js::string_of) {
            Some(data) => {
                save_png(local, out, &data).await?;
                Ok(None)
            }
            None => Ok(Some(error_or(Some(&j), status.to_string()))),
        }
    }
    .await;
    match relay {
        Ok(None) => return Ok("chatgpt".to_owned()),
        Ok(Some(e)) | Err(e) => errs.push(format!("opencodex: {e}")),
    }
    let gem = key_of(local, "GEMINI_API_KEY").await;
    if !gem.is_empty() {
        let body = json!({
            "contents": [{ "parts": [{ "text": prompt }] }],
            "generationConfig": { "responseModalities": ["IMAGE"], "imageConfig": { "aspectRatio": opts.aspect } },
        });
        let tried: Result<Option<String>, String> = async {
            let (_, text) = post_json(&format!("{GEMINI}?key={gem}"), &[("content-type", "application/json")], &body).await?;
            let j: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let parts = j.pointer("/candidates/0/content/parts").and_then(Value::as_array);
            let data = parts
                .and_then(|p| p.iter().find(|p| p.get("inlineData").is_some_and(crate::js::truthy)))
                .and_then(|p| p.pointer("/inlineData/data"))
                .filter(|d| crate::js::truthy(d))
                .map(crate::js::string_of);
            match data {
                Some(data) => {
                    save_png(local, out, &data).await?;
                    Ok(None)
                }
                None => Ok(Some(error_or(Some(&j), "no image".to_owned()))),
            }
        }
        .await;
        match tried {
            Ok(None) => return Ok("gemini".to_owned()),
            Ok(Some(e)) | Err(e) => errs.push(format!("gemini: {e}")),
        }
    }
    let oai = key_of(local, "OPENAI_API_KEY").await;
    if !oai.is_empty() {
        let body = json!({ "model": "gpt-image-1", "size": opts.size, "quality": "medium", "prompt": prompt });
        let auth = format!("Bearer {oai}");
        let tried: Result<Option<String>, String> = async {
            let (status, text) = post_json(OPENAI_IMAGES, &[("content-type", "application/json"), ("authorization", &auth)], &body).await?;
            let j: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            match j.pointer("/data/0/b64_json").filter(|d| crate::js::truthy(d)).map(crate::js::string_of).filter(|_| (200..300).contains(&status)) {
                Some(data) => {
                    std::fs::write(out, b64(&data)).map_err(|e| e.to_string())?;
                    Ok(None)
                }
                None => Ok(Some(error_or(Some(&j), status.to_string()))),
            }
        }
        .await;
        match tried {
            Ok(None) => return Ok("openai".to_owned()),
            Ok(Some(e)) | Err(e) => errs.push(format!("openai: {e}")),
        }
    }
    // no image model with quota: the chat model draws an SVG, QuickLook
    // rasterizes it
    if !settings.anthropic() {
        return Err(format!("no image model available (add GEMINI or OPENAI quota){}", if errs.is_empty() { String::new() } else { format!(" — {}", errs.join("; ")) }));
    }
    if !errs.is_empty() {
        let short: Vec<String> = errs.iter().map(|e| crate::js::slice_utf16(e.split('\n').next().unwrap_or(""), 160)).collect();
        eprintln!("  image: falling back to Claude SVG ({})", short.join("; "));
    }
    let hint = opts.svg_hint.clone().unwrap_or_else(|| format!("Draw this as a single polished SVG illustration (viewBox 0 0 512 512, full-bleed, rich gradients): {prompt}."));
    let body = json!({
        "model": settings.model,
        "max_tokens": 8000,
        "messages": [{ "role": "user", "content": format!("{hint} Reply with only the <svg>…</svg> markup.") }],
    });
    let m = api.create(turn, body).await.map_err(|e| parse_llm(&e).1.to_owned())?;
    let text: String = m
        .get("content")
        .and_then(Value::as_array)
        .map(|c| c.iter().map(|b| b.get("text").filter(|t| crate::js::truthy(t)).map(crate::js::string_of).unwrap_or_default()).collect())
        .unwrap_or_default();
    let svg = match (text.find("<svg"), text.rfind("</svg>")) {
        (Some(a), Some(b)) if b >= a => &text[a..b + 6],
        _ => return Err("the illustrator returned no SVG".to_owned()),
    };
    // QuickLook renders SVG through WebKit (gradients, filters intact)
    let out_s = out.display().to_string();
    let svg_path = format!("{}.svg", out_s.strip_suffix(".png").unwrap_or(&out_s));
    std::fs::write(&svg_path, svg).map_err(|e| e.to_string())?;
    let dir = Path::new(&svg_path).parent().map(|d| d.display().to_string()).unwrap_or_else(|| ".".to_owned());
    run(local, "qlmanage", &["-t", "-s", "1024", "-o", &dir, &svg_path]).await?;
    run(local, "mv", &[&format!("{svg_path}.png"), &out_s]).await?;
    std::fs::remove_file(&svg_path).map_err(|e| e.to_string())?;
    Ok("claude-svg".to_owned())
}

/// `generateAvatarImage(description, out)`.
pub async fn generate_avatar_image(settings: &Settings, api: &dyn ModelApi, turn: &TurnCtx, local: &dyn LocalTools, description: &str, out: &Path) -> Result<String, String> {
    let opts = ImageOpts {
        svg_hint: Some(format!("Draw a polished, detailed flat-illustration profile avatar as a single SVG (viewBox 0 0 512 512, square, full-bleed colored background, the subject centered and filling most of the frame, rich shading with gradients). Subject: {description}.")),
        ..Default::default()
    };
    generate_image(settings, api, turn, local, &format!("A square profile avatar image, centered subject filling the frame, clean simple background: {description}"), out, &opts).await
}
