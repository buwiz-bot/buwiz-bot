//! The artifact tools (tools.mjs `toolGenerateApp`, `toolGenerateImage`,
//! `toolArtifact`, `app_icon`, `app_details_update`, `artOf`,
//! `imageFileOf`, `imageModule`): a card checked by nsh and written as a
//! version through the pipeline (5.7a, `hub/pipeline`), its placeholder
//! the one the stream drew.
//!
//! The Library's side (5.7b: `genAppIcon`, `appInfo`, `updateAppMeta`) is
//! the [`AppMetaSeam`] in `ctx.ext`, which 5.7b's `library::appmeta` fills
//! (`routes::apps::install`). `setAppCols` and `iconPath` are its own.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use super::{Out, Run};
use crate::hub::ctx::HubCtx;
use crate::hub::library::appmeta;
use crate::hub::pipeline::{self, versions, Dirs};
use crate::hub::store::{uuidv7, Turn};
use crate::hub::types::BoxFuture;
use crate::js;

/// appmeta.mjs's icon and details (plan 5.7b).
pub trait AppMeta: Send + Sync {
    /// `genAppIcon(name, prompt, model)`: the icon drawn; the backend's name.
    fn gen_icon(&self, ctx: Arc<HubCtx>, name: String, prompt: String, model: String) -> BoxFuture<'static, Result<String, String>>;
    /// `updateAppMeta(name, meta)` then `appInfo(name)`: the app's details.
    fn update(&self, ctx: Arc<HubCtx>, name: String, meta: Value) -> BoxFuture<'static, Result<Value, String>>;
}

#[derive(Clone)]
pub struct AppMetaSeam(pub Arc<dyn AppMeta>);

/// `clampDim(n, lo, hi)`.
fn clamp_dim(n: Option<&Value>, lo: f64, hi: f64) -> i64 {
    let v = crate::hub::store::json::js_number(n);
    let v = (v + 0.5).floor();
    if v >= lo && v <= hi {
        v as i64
    } else {
        0
    }
}

fn text_of(v: Option<&Value>) -> String {
    v.filter(|x| js::truthy(x)).map(js::string_of).unwrap_or_default()
}

/// `artOf(input)`: the card's declared title, description and size, or
/// `None` when it declared none.
pub fn art_of(input: &Value) -> Option<Value> {
    let lay = input.get("layout").filter(|l| js::truthy(l));
    let dim = |k: &str| lay.and_then(|l| l.get(k)).filter(|v| !v.is_null()).or_else(|| input.get(k));
    let title = js::slice_utf16(text_of(input.get("title")).trim_matches(js::is_space), 60);
    let desc = js::slice_utf16(text_of(input.get("desc")).trim_matches(js::is_space), 140);
    let (w, h) = (clamp_dim(dim("width"), 60.0, 2048.0), clamp_dim(dim("height"), 60.0, 2048.0));
    (!title.is_empty() || !desc.is_empty() || w != 0 || h != 0).then(|| json!({"title": if title.is_empty() { Value::Null } else { json!(title) }, "desc": desc, "w": w, "h": h}))
}

fn is_card(s: &str) -> bool {
    s.strip_prefix('R').is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

/// `verFile(name, v)` for a JS number `v`.
fn ver_file(ctx: &HubCtx, name: &str, v: f64) -> Option<PathBuf> {
    versions::ver_file(&Dirs::of(&ctx.data), name, &js::number(v))
}

/// `imageFileOf(name, v)`: the generated image an image card shows.
pub fn image_file_of(ctx: &HubCtx, name: &str, v: Option<f64>) -> Option<String> {
    let path = match v {
        Some(v) => ver_file(ctx, name, v)?,
        None => ctx.store.gen_dir().join(format!("{name}.tsx")),
    };
    let text = std::fs::read_to_string(path).ok()?;
    let at = text.find("asset:/generated/assets/img-")? + "asset:/generated/assets/".len();
    let n = text[at..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')).unwrap_or(text.len() - at);
    Some(text[at..at + n].to_owned())
}

/// `imageModule(file, prompt, w, h)`: an image card's module.
pub fn image_module(file: &str, w: u32, h: u32) -> String {
    let ar = if w > 0 && h > 0 { format!(", aspectRatio: {:.4}", w as f64 / h as f64) } else { String::new() };
    format!(
        "import {{ signal, Text, View }} from \"nsh\";\nimport {{ Image }} from \"nsh/image\";\n\nexport default function R() {{\n  const failed = signal(false);\n  return (\n    <View class=\"gap-2\">\n      <If when={{!failed()}}>\n        <Image src=\"asset:/generated/assets/{file}\" resizeMode=\"cover\" onError={{() => failed.set(true)}} style={{{{ width: \"100%\"{ar}, borderRadius: 12 }}}} />\n      </If>\n      <If when={{failed()}}>\n        <Text class=\"text-sm text-zinc-500\">image failed to load</Text>\n      </If>\n    </View>\n  );\n}}\n"
    )
}

/// state.mjs `cardTurn(thread, name, v)`: the card of that version, else
/// the latest card of that name (never a building one).
fn card_turn(ctx: &HubCtx, thread: &str, name: &str, v: Option<i64>) -> Option<String> {
    ctx.store.read(|s| {
        let t = s.thread(thread)?;
        let cards: Vec<&Turn> = t.turns.iter().filter(|u| u.role == "ai" && u.name.as_deref() == Some(name) && !u.building).collect();
        v.and_then(|v| cards.iter().rev().find(|u| u.v == Some(v))).or(cards.last()).and_then(|u| u.id.clone())
    })
}

/// `markRevising(job, turn)`: the card covered while its revise runs.
pub fn mark_revising(run: &Run, name: &str, v: Option<i64>) {
    let card = card_turn(&run.ctx, &run.turn.thread, name, v);
    let now = run.ctx.jobs.with(run.job, |j| j.revising.clone()).flatten();
    if now.is_some() && now == card {
        return;
    }
    clear_revising(run);
    let Some(card) = card else { return };
    run.ctx.store.touch(|s| {
        for t in s.list.iter_mut() {
            for u in t.turns.iter_mut().filter(|u| u.id.as_deref() == Some(card.as_str())) {
                u.revising = true;
            }
        }
    });
    run.ctx.jobs.with(run.job, |j| j.revising = Some(card));
}

/// `clearRevising(job)`.
pub fn clear_revising(run: &Run) {
    let Some(card) = run.ctx.jobs.with(run.job, |j| j.revising.take()).flatten() else { return };
    run.ctx.store.touch(|s| {
        for t in s.list.iter_mut() {
            for u in t.turns.iter_mut().filter(|u| u.id.as_deref() == Some(card.as_str())) {
                u.revising = false;
            }
        }
    });
}

/// The model's `const S = "tipcalc"` (a self-named state key) pinned to the
/// module's real name, first match only (only when the code calls /state).
fn pin_state_key(code: &str, name: &str) -> String {
    const KEYS: [&str; 8] = ["WIDGET_NAME", "WIDGET", "MODULE", "NAME", "SELF", "KEY", "MOD", "S"];
    let mut from = 0;
    while let Some(p) = code[from..].find("const") {
        let at = from + p;
        from = at + 5;
        let rest = &code[at + 5..];
        let ws = rest.len() - rest.trim_start_matches(js::is_space).len();
        if ws == 0 {
            continue;
        }
        let rest = &rest[ws..];
        let Some(key) = KEYS.iter().find(|k| rest.starts_with(*k) && rest[k.len()..].starts_with(|c: char| js::is_space(c) || c == '=')) else { continue };
        let rest = rest[key.len()..].trim_start_matches(js::is_space);
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let rest = rest.trim_start_matches(js::is_space);
        let Some(q) = rest.chars().next().filter(|c| matches!(c, '"' | '\'' | '`')) else { continue };
        let body = &rest[1..];
        let word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
        let n = body.find(|c: char| !word(c)).unwrap_or(body.len());
        if !(2..=41).contains(&n) || !body.starts_with(|c: char| c.is_ascii_lowercase()) || !body[n..].starts_with(q) {
            continue;
        }
        let lit_start = code.len() - rest.len();
        let lit_end = lit_start + 1 + n + 1;
        return format!("{}{}{}", &code[..lit_start], js::quote(name), &code[lit_end..]);
    }
    code.to_owned()
}

/// Drops a building turn this tool made, and the job's hold on it.
fn drop_turn(run: &Run, id: &str) {
    run.ctx.store.touch(|s| {
        if let Some(t) = s.thread_mut(&run.turn.thread) {
            t.turns.retain(|u| u.id.as_deref() != Some(id));
        }
    });
    run.ctx.jobs.with(run.job, |j| {
        if j.turn.as_deref() == Some(id) {
            j.turn = None;
        }
    });
}

/// `toolGenerateApp(job, thread, input)` (and `revise_app`, whose input
/// names the card): the code checked by nsh, then a new version.
pub async fn generate_app(run: &Run, input: &Value) -> Result<Out, String> {
    let ctx = &run.ctx;
    let mut code = text_of(input.get("code")).trim_matches(js::is_space).to_owned();
    if !code.contains("export default") && has_function_r(&code) {
        code.push_str("\nexport default R;\n");
    }
    if !code.contains("export default") || !code.contains('<') {
        return Ok(Out::err("not a module: it needs `export default` on a component returning JSX.".into()));
    }
    let rev = Some(text_of(input.get("name"))).filter(|n| is_card(n));
    if let Some(r) = &rev {
        if !ctx.store.gen_dir().join(format!("{r}.tsx")).exists() {
            return Ok(Out::err(format!("no such artifact {r}")));
        }
    }
    let name = match &rev {
        Some(r) => r.clone(),
        None => format!("R{}", ctx.store.write(|s| s.next_module_seq())?),
    };
    run.set_job("checking", "Checking the code");
    let mut code = code.split("__NAME__").collect::<Vec<_>>().join(&name);
    if code.contains("/state/") {
        code = pin_state_key(&code, &name);
    }
    let code = pipeline::imports::fix_imports(&code);
    let dirs = Dirs::of(&ctx.data);
    let _ = std::fs::create_dir_all(&dirs.stage);
    std::fs::write(dirs.stage.join(format!("{name}.tsx")), &code).map_err(|e| e.to_string())?;
    // the card exists before the check: a placeholder whose status mirrors
    // the job; the one drawn while the code streamed becomes it
    let pre = run.state().pending_app.take().filter(|id| ctx.store.read(|s| s.thread(&run.turn.thread).is_some_and(|t| t.turn(id).is_some())));
    let id = pre.clone().unwrap_or_else(uuidv7);
    let label = run.label();
    let caption = input.get("caption").filter(|c| js::truthy(c)).map(js::string_of).unwrap_or_default();
    let pre_art = pre.as_ref().and_then(|p| ctx.store.read(|s| s.thread(&run.turn.thread).and_then(|t| t.turn(p)).and_then(|(_, u)| u.art.clone())));
    let art = art_of(input).or(pre_art).unwrap_or(Value::Null);
    let card = Turn {
        role: "ai".into(),
        id: Some(id.clone()),
        name: Some(name.clone()),
        building: true,
        status: Some("checking types…".into()),
        label: Some(label.clone()),
        caption: Some(caption.clone()),
        art: Some(art),
        ..Default::default()
    };
    // a revise's card lands only once it passes; a new card shows now, in
    // the placeholder's place when there is one
    let mut held = rev.is_some().then(|| card.clone());
    ctx.store.touch(|s| {
        let Some(t) = s.thread_mut(&run.turn.thread) else { return };
        let at = t.turns.iter().position(|u| u.id.as_deref() == Some(id.as_str()));
        match (at, rev.is_some()) {
            (Some(i), true) => {
                t.turns.remove(i);
            }
            (Some(i), false) => t.turns[i] = card,
            (None, true) => {}
            (None, false) => t.turns.push(card),
        }
    });
    match &rev {
        Some(r) => {
            let base = crate::hub::store::json::js_number(input.get("base_v"));
            mark_revising(run, r, (base.is_finite() && base != 0.0).then_some(base as i64));
        }
        None => {
            ctx.jobs.with(run.job, |j| j.turn = Some(id.clone()));
        }
    }
    let nsh = ctx.nsh.clone().unwrap_or_else(|| Arc::new(pipeline::nsh::NshCli::from_env()));
    match pipeline::nsh::check_candidate(nsh.as_ref(), &dirs.stage, &name).await {
        // BUG-compatible: a missing nsh answers "", which reads as a pass
        // here (`if (err)`), unlike the legacy path's `err === null`
        Err(e) if !e.is_empty() => {
            drop_turn(run, &id);
            return Ok(Out::err(format!("nsh check rejected {name}:\n{}\nFix the module and call generate_app again with the corrected code.", js::slice_utf16(&e, 3000))));
        }
        _ => {}
    }
    // revising off a specific version branches; default parent is the head
    let head_before = if rev.is_some() { versions::head_of(ctx, &name)? } else { None };
    let base = crate::hub::store::json::js_number(input.get("base_v"));
    let base_i = if base.is_finite() && base != 0.0 { base } else { 0.0 };
    let parent = match &rev {
        Some(_) => {
            let has = ctx.db.with(|c| c.query_row("SELECT v FROM versions WHERE name=? AND v=?", rusqlite::params![name, base_i], |_| Ok(())).optional())?.is_some();
            if has {
                Some(base as i64)
            } else {
                head_before
            }
        }
        None => None,
    };
    let branched = rev.is_some() && parent.is_some() && parent != head_before;
    // the apps row lands first, so the version file is named by its slug
    let prior: (Option<String>, Option<String>) = ctx.db.with(|c| c.query_row("SELECT title,file FROM apps WHERE name=?", [&name], |r| Ok((r.get(0)?, r.get(1)?))).optional())?.unwrap_or((None, None));
    let title = Some(js::slice_utf16(text_of(input.get("title")).trim_matches(js::is_space), 60)).filter(|t| !t.is_empty()).or(prior.0.filter(|t| !t.is_empty()));
    let file = Some(crate::hub::store::slug_file(&text_of(input.get("slug")))).filter(|f| !f.is_empty()).or(prior.1.filter(|f| !f.is_empty()));
    let desc_src = if js::truthy(input.get("description").unwrap_or(&Value::Null)) {
        text_of(input.get("description"))
    } else if rev.is_some() {
        String::new()
    } else {
        text_of(input.get("desc"))
    };
    let description = js::slice_utf16(desc_src.trim_matches(js::is_space), 280);
    let mut cols = vec![("title", title), ("file", file)];
    if !description.is_empty() {
        cols.push(("description", Some(description)));
    }
    let cols: Vec<(&str, rusqlite::types::Value)> = cols.into_iter().map(|(k, v)| (k, v.map_or(rusqlite::types::Value::Null, rusqlite::types::Value::Text))).collect();
    appmeta::set_app_cols(ctx, &name, &cols)?;
    // BUG: toolGenerateApp records `job.prompt`, which no job carries, so a
    // tool-made version's prompt is the caption or NULL (LOOP-04)
    let prompt = caption.clone();
    let branch = input.get("branch_name").filter(|b| !b.is_null()).map(js::string_of);
    let (v, _chunks) = versions::write_version(ctx, &name, &code, parent, Some(&prompt), branch.as_deref())?;
    // the placeholder becomes the real card, pinned to this version
    ctx.store.write(|s| {
        let Some(t) = s.thread_mut(&run.turn.thread) else { return };
        match held.take() {
            Some(mut u) => {
                u.v = Some(v);
                u.building = false;
                u.status = None;
                t.turns.push(u);
            }
            None => {
                if let Some(u) = t.turns.iter_mut().find(|u| u.id.as_deref() == Some(id.as_str())) {
                    u.v = Some(v);
                    u.building = false;
                    u.status = None;
                }
            }
        }
    })?;
    ctx.jobs.with(run.job, |j| {
        if j.turn.as_deref() == Some(id.as_str()) {
            j.turn = None;
        }
    });
    if rev.is_some() {
        clear_revising(run);
    }
    run.state().cards += 1;
    ctx.store.write_index();
    println!("✓ {name}@v{v}.tsx via {} (tool)", run.label());
    // the icon draws in the background: never blocks or fails the result
    let icon_prompt = text_of(input.get("icon_prompt")).trim_matches(js::is_space).to_owned();
    if !appmeta::icon_path(ctx, &name).exists() || (rev.is_some() && !icon_prompt.is_empty()) {
        if let Some(m) = ctx.ext.get::<AppMetaSeam>().cloned() {
            let model = run.turn.model.as_ref().map(|m| m.model.clone()).filter(|m| !m.is_empty()).unwrap_or_else(|| run.settings.model.clone());
            let (ctx2, name2) = (ctx.clone(), name.clone());
            tokio::spawn(async move {
                if let Err(e) = m.0.gen_icon(ctx2, name2.clone(), icon_prompt, model).await {
                    eprintln!("  icon {name2}: {}", js::slice_utf16(&e, 200));
                }
            });
        }
    }
    let note = if branched {
        let named = branch.map(|b| format!(" named \"{}\"", js::slice_utf16(b.trim_matches(js::is_space), 40))).filter(|_| input.get("branch_name").is_some_and(js::truthy)).unwrap_or_default();
        format!(" — this is a new branch off v{}{named}; mention the fork to the user (they can rename it in Library history)", parent.unwrap_or(0))
    } else {
        String::new()
    };
    Ok(Out::ok(format!("mounted as {name} v{v}{note} — the card is live in the conversation. Library now launches v{v}; earlier versions stay on the tree.")))
}

/// `/function\s+R\s*\(/`.
fn has_function_r(code: &str) -> bool {
    code.match_indices("function").any(|(i, _)| {
        let rest = &code[i + 8..];
        let ws = rest.len() - rest.trim_start_matches(js::is_space).len();
        ws > 0 && rest[ws..].strip_prefix('R').is_some_and(|r| r.trim_start_matches(js::is_space).starts_with('('))
    })
}

/// gpt-image's shapes.
fn aspect_size(a: &str) -> Option<(u32, u32)> {
    match a {
        "1:1" => Some((1024, 1024)),
        "3:2" => Some((1536, 1024)),
        "2:3" => Some((1024, 1536)),
        _ => None,
    }
}

/// `aspectOfFile(path)`: the shape closest to the image's.
async fn aspect_of_file(ctx: &HubCtx, path: &Path) -> &'static str {
    let m = crate::hub::agent::files::image_size(ctx, path).await;
    let (w, h) = (m.get("width").and_then(Value::as_f64).unwrap_or(0.0), m.get("height").and_then(Value::as_f64).unwrap_or(0.0));
    if w > 0.0 && h > 0.0 {
        if w > h * 1.15 {
            return "3:2";
        }
        if h > w * 1.15 {
            return "2:3";
        }
    }
    "1:1"
}

/// `toolGenerateImage(job, thread, input)`: a real image as a card, or an
/// image card's next version (`edit_of`).
pub async fn generate_image(run: &Run, input: &Value) -> Result<Out, String> {
    let ctx = &run.ctx;
    let prompt = text_of(input.get("prompt")).trim_matches(js::is_space).to_owned();
    if prompt.is_empty() {
        return Ok(Out::err("prompt is required".into()));
    }
    run.set_job("generating", "Sketching something up");
    let edit_of = Some(text_of(input.get("edit_of"))).filter(|e| is_card(e));
    let src = edit_of.as_deref().and_then(|e| image_file_of(ctx, e, None));
    if let (Some(e), None) = (&edit_of, &src) {
        return Ok(Out::err(format!("{e} is not an image card")));
    }
    let assets = ctx.store.gen_dir().join("assets");
    let given = input.get("aspect").and_then(Value::as_str).filter(|a| aspect_size(a).is_some());
    let aspect = match (given, &src) {
        (Some(a), _) => a,
        (None, Some(s)) => aspect_of_file(ctx, &assets.join(s)).await,
        _ => "1:1",
    };
    let (w, h) = aspect_size(aspect).unwrap_or((1024, 1024));
    let mut art = art_of(input).unwrap_or_else(|| json!({"title": null, "desc": ""}));
    if let (Some(e), true) = (&edit_of, art["title"].is_null()) {
        art["title"] = ctx.store.read(|s| {
            s.thread(&run.turn.thread)
                .and_then(|t| {
                    t.turns
                        .iter()
                        .rev()
                        .find(|u| u.name.as_deref() == Some(e.as_str()) && u.art.as_ref().is_some_and(|a| a.get("title").is_some_and(js::truthy)))
                        .map(|u| u.art.as_ref().unwrap()["title"].clone())
                })
                .unwrap_or(Value::Null)
        });
    }
    art["w"] = json!(w);
    art["h"] = json!(h);
    art["kind"] = json!("image");
    let file = format!("img-{}.png", crate::hub::store::now_ms());
    art["file"] = json!(file);
    let id = uuidv7();
    let turn = Turn {
        role: "ai".into(),
        id: Some(id.clone()),
        building: true,
        status: Some("generating image…".into()),
        label: Some("image".into()),
        caption: Some(input.get("caption").filter(|c| js::truthy(c)).map(js::string_of).unwrap_or_default()),
        art: Some(art.clone()),
        ..Default::default()
    };
    ctx.store.touch(|s| {
        if let Some(t) = s.thread_mut(&run.turn.thread) {
            t.turns.push(turn);
        }
    });
    ctx.jobs.with(run.job, |j| j.turn = Some(id.clone()));
    let _ = std::fs::create_dir_all(&assets);
    let local = crate::helper::local::tools(ctx);
    let drawn = match (&src, &ctx.model) {
        (Some(s), _) => crate::hub::model::images::edit_image(&run.settings, local.as_ref(), &assets.join(s), &prompt, &assets.join(&file), &format!("{w}x{h}")).await,
        (None, Some(api)) => {
            let opts = crate::hub::model::images::ImageOpts { size: format!("{w}x{h}"), aspect: aspect.to_owned(), ..Default::default() };
            crate::hub::model::images::generate_image(&run.settings, api.as_ref(), &run.turn, local.as_ref(), &prompt, &assets.join(&file), &opts).await
        }
        (None, None) => Err("no model client".into()),
    };
    let backend = match drawn {
        Ok(b) => b,
        Err(e) => {
            drop_turn(run, &id);
            return Ok(Out::err(format!("image generation failed: {}", js::slice_utf16(&e, 300))));
        }
    };
    let name = match &edit_of {
        Some(e) => e.clone(),
        None => format!("R{}", ctx.store.write(|s| s.next_module_seq())?),
    };
    let parent = match &edit_of {
        Some(e) => versions::head_of(ctx, e)?,
        None => None,
    };
    let (v, _) = versions::write_version(ctx, &name, &image_module(&file, w, h), parent, Some(&prompt), None)?;
    art["via"] = json!(backend);
    ctx.store.touch(|s| {
        if let Some(u) = s.thread_mut(&run.turn.thread).and_then(|t| t.turns.iter_mut().find(|u| u.id.as_deref() == Some(id.as_str()))) {
            u.name = Some(name.clone());
            u.v = Some(v);
            u.art = Some(art.clone());
            u.building = false;
            u.status = None;
        }
    });
    ctx.jobs.with(run.job, |j| {
        if j.turn.as_deref() == Some(id.as_str()) {
            j.turn = None;
        }
    });
    run.state().cards += 1;
    println!("✓ {name}.tsx image via {backend} ({})", js::slice_utf16(&prompt, 50));
    Ok(Out::ok(format!("image generated and mounted as {name} (asset:/generated/assets/{file}) — to embed it inside a widget, use <Image src=\"asset:/generated/assets/{file}\" …>.")))
}

/// A POST to this broker's front door (its public port, D34).
pub(crate) async fn self_post(path: &str, body: &Value) -> Value {
    let port = crate::hub::market::node_paths().1;
    let Ok(url) = url::Url::parse(&format!("http://127.0.0.1:{port}{path}")) else { return json!({}) };
    let req = crate::helper::http::Req::post(url, js::stringify(body).into_bytes()).header("Content-Type", "application/json");
    match crate::helper::http::send(&req).await {
        Ok(r) => r.text().await.ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({})),
        Err(e) => {
            eprintln!("  self-call {path}: {e}");
            json!({})
        }
    }
}

/// `toolArtifact(block, input)`: launch, close, promote, demote, set a
/// version, read the code.
pub async fn artifact_tool(run: &Run, tool: &str, input: &Value) -> Result<Out, String> {
    let ctx = &run.ctx;
    let name = input.get("name").filter(|n| js::truthy(n)).map(js::string_of).unwrap_or_default();
    if !is_card(&name) {
        return Ok(Out::err(format!("bad artifact name {name}")));
    }
    let err = |r: &Value| r.get("error").filter(|e| js::truthy(e)).map(js::string_of);
    let ok = |r: &Value| r.get("ok").is_some_and(js::truthy);
    let v_in = input.get("v").filter(|v| js::truthy(v));
    match tool {
        "launch_app" => {
            let mut pop = json!({"name": name});
            if let Some(v) = v_in {
                pop["v"] = json!(crate::hub::store::json::js_number(Some(v)));
            }
            let r = self_post("/popout", &pop).await;
            let shown = v_in.map(|v| format!(" v{}", js::string_of(v))).unwrap_or_default();
            Ok(if ok(&r) { Out::ok(format!("{name}{shown} opened in its own window.")) } else { Out::err(format!("launch failed: {}", err(&r).unwrap_or_else(|| "no such module".into()))) })
        }
        "close_app" => Ok(Out::ok(crate::hub::routes::popout::close_app(ctx, &name))),
        "promote_app" => {
            let r = self_post("/publish", &json!({"name": name})).await;
            Ok(if ok(&r) { Out::ok(format!("{name} is now in Mini Apps.")) } else { Out::err(format!("promote failed: {}", err(&r).unwrap_or_else(|| "?".into()))) })
        }
        "demote_app" => {
            let r = self_post("/unpublish", &json!({"name": name})).await;
            Ok(if ok(&r) { Out::ok(format!("{name} removed from Mini Apps.")) } else { Out::err(format!("demote failed: {}", err(&r).unwrap_or_else(|| "?".into()))) })
        }
        "set_version" => {
            let v = crate::hub::store::json::js_number(input.get("v"));
            let moved = v.fract() == 0.0 && v.is_finite() && versions::set_head(ctx, &name, v as i64)?.is_some();
            Ok(if moved {
                Out::ok(format!("{name} now points at v{} — Library launches it; later versions stay on the tree.", js::number(v)))
            } else {
                Out::err(format!("no version {} on {name}", js::number(v)))
            })
        }
        "view_code" => {
            let v = match v_in {
                Some(v) => Some(crate::hub::store::json::js_number(Some(v))),
                None => versions::head_of(ctx, &name)?.map(|v| v as f64),
            };
            let file = match v {
                Some(v) if v != 0.0 && !v.is_nan() => ver_file(ctx, &name, v),
                _ => Some(ctx.store.gen_dir().join(format!("{name}.tsx"))),
            };
            let shown = v.map(js::number).unwrap_or_default();
            let Some(text) = file.filter(|f| f.exists()).and_then(|f| std::fs::read_to_string(f).ok()) else {
                let at = v.filter(|v| *v != 0.0 && !v.is_nan()).map(|v| format!("@v{}", js::number(v))).unwrap_or_default();
                return Ok(Out::err(format!("no source for {name}{at}")));
            };
            Ok(Out::ok(format!("<code name=\"{name}@v{shown}\">\n{}\n</code>", js::slice_utf16(&text, 60000))))
        }
        _ => Ok(Out::err(format!("unknown artifact tool {tool}"))),
    }
}

/// `app_icon`: the icon redrawn now (5.7b's `genAppIcon`).
pub async fn app_icon(run: &Run, input: &Value) -> Result<Out, String> {
    let name = input.get("name").filter(|n| !n.is_null()).map(js::string_of).unwrap_or_default();
    run.set_job("generating", "Sketching something up");
    let Some(m) = run.ctx.ext.get::<AppMetaSeam>().cloned() else { return Err("not ported: app icons (5.7b)".into()) };
    let model = run.turn.model.as_ref().map(|m| m.model.clone()).filter(|m| !m.is_empty()).unwrap_or_else(|| run.settings.model.clone());
    // genAppIcon reads it as `String(prompt || "")`
    let prompt = text_of(input.get("prompt"));
    let backend = m.0.gen_icon(run.ctx.clone(), name.clone(), prompt, model).await?;
    Ok(Out::ok(format!("{} has a new icon (drawn by {backend}).", crate::hub::store::json::template(input.get("name")))))
}

/// `app_details_update`: the app's details without touching its code.
pub async fn app_details_update(run: &Run, input: &Value) -> Result<Out, String> {
    let Some(m) = run.ctx.ext.get::<AppMetaSeam>().cloned() else { return Err("not ported: app details (5.7b)".into()) };
    let name = input.get("name").filter(|n| !n.is_null()).map(js::string_of).unwrap_or_default();
    let mut meta = serde_json::Map::new();
    for (k, v) in input.as_object().into_iter().flatten() {
        match k.as_str() {
            "name" => {}
            "width" => {
                meta.insert("w".into(), v.clone());
            }
            "height" => {
                meta.insert("h".into(), v.clone());
            }
            _ => {
                meta.insert(k.clone(), v.clone());
            }
        }
    }
    let i = m.0.update(run.ctx.clone(), name, Value::Object(meta)).await?;
    let t = |k: &str| crate::hub::store::json::template(i.get(k));
    Ok(Out::ok(format!(
        "{} details: title \"{}\", file {}, description \"{}\", size {}×{}",
        crate::hub::store::json::template(input.get("name")),
        t("title"),
        t("file"),
        t("description"),
        t("w"),
        t("h")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_art_and_state_keys_read_as_tools_mjs_does() {
        assert_eq!(art_of(&json!({})), None);
        assert_eq!(art_of(&json!({"title": " Tiny Tip ", "layout": {"width": 320, "height": 200}})), Some(json!({"title": "Tiny Tip", "desc": "", "w": 320, "h": 200})));
        assert_eq!(art_of(&json!({"width": 59.4, "height": 3000})), None);
        assert_eq!(art_of(&json!({"desc": "d", "width": "100"})), Some(json!({"title": null, "desc": "d", "w": 100, "h": 0})));
        assert_eq!(pin_state_key("const S = \"tipcalc\"; fetch(`/state/${S}`)", "R7"), "const S = \"R7\"; fetch(`/state/${S}`)");
        assert_eq!(pin_state_key("const KEY='my-app'", "R7"), "const KEY=\"R7\"");
        assert_eq!(pin_state_key("const S = \"Tip\"", "R7"), "const S = \"Tip\"");
        assert!(image_module("img-1.png", 1536, 1024).contains("style={{ width: \"100%\", aspectRatio: 1.5000, borderRadius: 12 }}"));
    }
}
