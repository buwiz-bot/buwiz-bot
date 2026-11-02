//! The bot routes (5.2; routes/bots.mjs): new, update, memories,
//! integrations, delete, the avatar images and shape, and the composer's
//! attachment picker. The Mac's pickers and image tools run through
//! `ctx.local`.
//!
//! `/avatar/gen` asks the model (5.3) through `ctx.model`; without one it
//! answers 501. `/avatar/image/gen` draws through the [`AvatarImage`] seam
//! in `ctx.ext` (5.3's image models); without it that kind answers 501.
//!
//! `/bot/new` and `/bot/update` check the avatar (`applyAvatar`: a kit
//! shape clears a custom outline). `/avatar/design` (avatar-design.mjs) asks
//! Design's model, or the one the picker sent, for three candidates and
//! checks them with `library::avatar_shape`; MOCK answers fixed ones.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use super::chat::{answer, card_names, ok, parse, ready, refused, sweep_modules, thrown, trimmed, wrote};
use crate::helper::local::{command, exec, tools};
use crate::hub::ctx::HubCtx;
use crate::hub::library::{avatar_shape, botctx};
use crate::hub::model::anthropic::parse_llm;
use crate::hub::model::{openai, Settings};
use crate::hub::library::groups::{now36, random36, time_id};
use crate::hub::store::json::{out, template, turns_json, Lookups};
use crate::hub::store::{bot_defaults, Bot};
use crate::hub::traits::LocalCommand;
use crate::hub::types::{not_ported, ModelChoice, BoxFuture, HubRequest, HubResponse, Pattern::Exact as X, Pattern::Prefix as P, TurnCtx, Verb};
use crate::hub::Registry;
use crate::js;

/// avatars.mjs `generateAvatarImage(description, out)`: draws the avatar
/// into `out`. Track B fills it in `ctx.ext`.
#[derive(Clone)]
pub struct AvatarImage(pub Arc<dyn Fn(Arc<HubCtx>, String, PathBuf) -> BoxFuture<'static, Result<(), String>> + Send + Sync>);

pub fn install(routes: &mut Registry) {
    routes.route(Verb::Post, X("/bot/new"), bot_new);
    routes.route(Verb::Post, X("/bot/update"), bot_update);
    routes.route(Verb::Post, X("/bot/memory"), bot_memory);
    routes.route(Verb::Post, X("/bot/integration"), bot_integration);
    routes.route(Verb::Post, X("/bot/delete"), bot_delete);
    routes.route(Verb::Post, X("/attach/pick"), attach_pick);
    routes.route(Verb::Post, P("/avatar/image/"), avatar_image);
    routes.route(Verb::Post, X("/avatar/gen"), avatar_gen);
    routes.route(Verb::Post, X("/avatar/design"), avatar_design);
}

/// routes/bots.mjs `applyAvatar(bot, {shape, color, outline})`: the
/// avatar a request leaves a bot with (`cur` its shape, colour and
/// outline), or the errors joined. A kit shape clears the outline, an
/// outline without a shape makes it "custom"; only the fields given are
/// checked.
fn apply_avatar(cur: (&str, &str, Option<&str>), b: &Value) -> Result<(String, String, Option<String>), String> {
    let shape = b.get("shape").filter(|v| js::truthy(v));
    let color = b.get("color").filter(|v| js::truthy(v));
    let outline = b.get("outline");
    let has_outline = outline.is_some_and(|v| !v.is_null() && v.as_str() != Some(""));
    // `None`: a value that is not a string
    let s: Option<&str> = match shape {
        Some(v) => v.as_str(),
        None if has_outline => Some("custom"),
        None => Some(cur.0),
    };
    let c: Option<&str> = match color {
        Some(v) => v.as_str(),
        None => Some(cur.1),
    };
    let o: Option<String> = if s == Some("custom") {
        if has_outline {
            outline.and_then(Value::as_str).map(|o| o.trim_matches(js::is_space).to_owned())
        } else {
            cur.2.map(str::to_owned)
        }
    } else {
        None
    };
    let errs = avatar_shape::validate_avatar(
        if shape.is_some() || has_outline || s == Some("custom") { s } else { Some("blob") },
        if color.is_some() { c } else { Some("#1084FE") },
        o.as_deref(),
    );
    if !errs.is_empty() {
        return Err(errs.join("; "));
    }
    let (s, c) = (s.unwrap_or_default().to_owned(), c.unwrap_or_default().to_owned());
    Ok((s, c, o.filter(|o| !o.is_empty())))
}

async fn bot_new(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    let b = match parse(&req.body) {
        Ok(b) => b,
        Err(e) => return refused(&e, false),
    };
    let name = match trimmed(&b, "name") {
        Ok(Some(n)) if !n.is_empty() => n,
        Ok(_) => return refused("bot needs a name", false),
        Err(e) => return refused(&e, false),
    };
    let (shape, color, outline) = match apply_avatar(("blob", "#1084FE", None), &b) {
        Ok(a) => a,
        Err(e) => return refused(&e, false),
    };
    let id = time_id("b");
    let l = wrote!(ctx.db.with(Lookups::read));
    let gen = ctx.store.gen_dir();
    let (tid, turns) = wrote!(ctx.store.write(|s| {
        let mut t = s.fresh_thread(&name);
        let tid = t.id.clone();
        let turns = turns_json(&mut t, &l, &gen);
        s.list.insert(0, t);
        s.bots.push(Bot { outline: outline.clone(), ..Bot::new(&id, &name, &shape, &color, Some(tid.clone())) });
        s.active = tid.clone();
        (tid, turns)
    }));
    let (d, i) = bot_defaults(&name);
    let mut bot = json!({"id": id, "name": name, "shape": shape, "color": color});
    if let Some(o) = &outline {
        bot["outline"] = json!(o);
    }
    for (k, v) in [("threadId", json!(tid)), ("description", json!(d)), ("instructions", json!(i)), ("memories", json!([])), ("integrations", json!([]))] {
        bot[k] = v;
    }
    ctx.store.write_index();
    ok(&json!({"ok": true, "bot": bot, "turns": turns}))
}

async fn bot_update(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    let b = match parse(&req.body) {
        Ok(b) => b,
        Err(e) => return refused(&e, false),
    };
    let id = b.get("id");
    let Some((bot_id, cur)) = ctx.store.read(|s| id.and_then(Value::as_str).and_then(|id| s.bot(id)).map(|b| (b.id.clone(), (b.shape.clone(), b.color.clone(), b.outline.clone())))) else {
        return refused(&format!("unknown bot {}", template(id)), false);
    };
    let avatar = if b.get("shape").is_some_and(js::truthy) || b.get("color").is_some_and(js::truthy) || b.get("outline").is_some() {
        match apply_avatar((&cur.0, &cur.1, cur.2.as_deref()), &b) {
            Ok(a) => Some(a),
            Err(e) => return refused(&e, false),
        }
    } else {
        None
    };
    let name = match trimmed(&b, "name") {
        Ok(n) => n.filter(|n| !n.is_empty()),
        Err(e) => return refused(&e, false),
    };
    let section = b.get("section").map(|v| js::string_of(v).trim_matches(js::is_space).to_owned());
    wrote!(ctx.store.write(|s| {
        let Some(bot) = s.bot_mut(&bot_id) else { return };
        if let Some((shape, color, outline)) = avatar {
            (bot.shape, bot.color, bot.outline) = (shape, color, outline);
        }
        if let Some(name) = name.filter(|n| *n != bot.name) {
            // untouched defaults follow a rename; edited ones stay
            let (was, now) = (bot_defaults(&bot.name), bot_defaults(&name));
            if bot.description.as_deref() == Some(was.0.as_str()) {
                bot.description = Some(now.0);
            }
            if bot.instructions.as_deref() == Some(was.1.as_str()) {
                bot.instructions = Some(now.1);
            }
            bot.name = name;
        }
        if let Some(v) = b.get("pinned") {
            bot.pinned = js::truthy(v);
        }
        if let Some(v) = b.get("hidden") {
            bot.hidden = js::truthy(v);
        }
        if let Some(v) = b.get("image") {
            bot.image = js::string_of(v);
        }
        if let Some(v) = b.get("crop") {
            bot.crop = v.clone();
        }
        if let Some(v) = b.get("model") {
            bot.model = js::string_of(v);
        }
        if let Some(v) = b.get("effort") {
            bot.effort = js::string_of(v);
        }
        if let Some(v) = b.get("fast") {
            bot.fast = js::truthy(v);
        }
        botctx::set_profile(bot, b.get("label"), b.get("description"), b.get("instructions"));
        if let Some(sec) = &section {
            bot.section = sec.clone();
        }
        let (name, thread) = (bot.name.clone(), bot.thread_id.clone());
        if let Some(sec) = section.filter(|x| !x.is_empty()) {
            if !s.sections.is_array() {
                s.sections = json!([]);
            }
            if let Some(a) = s.sections.as_array_mut() {
                if !a.contains(&json!(sec)) {
                    a.push(json!(sec));
                }
            }
        }
        if let Some(t) = thread.and_then(|id| s.list.iter_mut().find(|t| t.id == id)) {
            t.title = name;
        }
    }));
    ok(&json!({"ok": true}))
}

async fn bot_memory(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    let b = match parse(&req.body) {
        Ok(b) => b,
        Err(e) => return refused(&e, true),
    };
    let id = b.get("id");
    let Some(bot_id) = ctx.store.read(|s| id.and_then(Value::as_str).and_then(|id| s.bot(id)).map(|b| b.id.clone())) else {
        return refused(&format!("unknown bot {}", template(id)), true);
    };
    let memory_id = b.get("memoryId").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
    let op = b.get("op").and_then(Value::as_str).unwrap_or_default().to_owned();
    let r = wrote!(ctx.store.write(|s| {
        let bot = s.bot_mut(&bot_id).ok_or_else(|| format!("unknown bot {bot_id}"))?;
        match op.as_str() {
            "add" => botctx::memory_add(bot, b.get("text")).map(|_| ()),
            "update" => botctx::memory_update(bot, &memory_id, b.get("text")),
            "remove" => botctx::memory_remove(bot, &memory_id),
            _ => Err(format!("unknown op {}", template(b.get("op")))),
        }?;
        Ok::<_, String>(bot.memories.clone())
    }));
    match r {
        Ok(memories) => ok(&json!({"ok": true, "memories": memories})),
        Err(e) => refused(&e, true),
    }
}

async fn bot_integration(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    let b = match parse(&req.body) {
        Ok(b) => b,
        Err(e) => return refused(&e, true),
    };
    let id = b.get("id");
    let Some(bot_id) = ctx.store.read(|s| id.and_then(Value::as_str).and_then(|id| s.bot(id)).map(|b| b.id.clone())) else {
        return refused(&format!("unknown bot {}", template(id)), true);
    };
    let spec = b.get("integration");
    let op = b.get("op").and_then(Value::as_str).unwrap_or_default().to_owned();
    let r: Result<(), String> = match op.as_str() {
        "add" => match ctx.db.with(|c| Ok(botctx::resolve_integration(&ctx, c, spec))) {
            Ok(Ok(hit)) => ctx.store.write(|s| {
                if let Some(bot) = s.bot_mut(&bot_id) {
                    botctx::integration_add(bot, &hit, "user");
                }
            }),
            Ok(Err(e)) | Err(e) => Err(e),
        },
        "remove" => match ctx.db.with(|c| Ok(botctx::plugin_servers(c))) {
            Ok(owners) => match ctx.store.write(|s| s.bot_mut(&bot_id).map(|bot| botctx::integration_remove(bot, spec, &owners))) {
                Ok(Some(r)) => r,
                Ok(None) => Ok(()),
                Err(e) => return thrown(&e),
            },
            Err(e) => Err(e),
        },
        _ => Err(format!("unknown op {}", template(b.get("op")))),
    };
    if let Err(e) = r {
        return refused(&e, true);
    }
    let Some(bot) = ctx.store.read(|s| s.bot(&bot_id).cloned()) else { return refused(&format!("unknown bot {bot_id}"), true) };
    let xs = wrote!(ctx.db.with(|c| Ok(botctx::integrations_of(&ctx, c, &bot))));
    ok(&json!({"ok": true, "integrations": xs}))
}

async fn bot_delete(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    let b = match parse(&req.body) {
        Ok(b) => b,
        Err(e) => return refused(&e, false),
    };
    let id = b.get("id");
    let Some((bot_id, thread, names)) = ctx.store.read(|s| {
        let bot = id.and_then(Value::as_str).and_then(|id| s.bot(id))?;
        let t = bot.thread_id.as_deref().and_then(|t| s.list.iter().find(|x| x.id == t));
        Some((bot.id.clone(), bot.thread_id.clone(), card_names(t)))
    }) else {
        return refused(&format!("unknown bot {}", template(id)), false);
    };
    if let Err(e) = sweep_modules(&ctx, &names) {
        return refused(&e, false);
    }
    wrote!(ctx.store.write(|s| {
        s.bots.retain(|b| b.id != bot_id);
        s.list.retain(|t| Some(&t.id) != thread.as_ref());
        if s.list.is_empty() {
            let t = s.fresh_thread("new chat");
            s.list.push(t);
        }
        if Some(&s.active) == thread.as_ref() {
            s.active = s.list[0].id.clone();
        }
    }));
    ctx.store.write_index();
    ok(&json!({"ok": true}))
}

/// `execFile(cmd, args, {timeout}, err => ok(!err))`.
async fn run_ok(ctx: &HubCtx, program: &str, args: &[&str], timeout_ms: u64) -> bool {
    let mut c = command(program, args);
    c.timeout = Some(std::time::Duration::from_millis(timeout_ms));
    matches!(tools(ctx).run(c).await, Ok(o) if o.code == Some(0))
}

/// `osa(script)`: `osascript -e <script>`, its output trimmed.
async fn osa(ctx: &HubCtx, script: &str) -> Result<String, String> {
    exec(ctx, LocalCommand { program: "osascript".into(), args: vec!["-e".into(), script.into()], ..Default::default() }).await.map(|o| o.trim().to_owned())
}

/// files.mjs `imageSize(path)`: `{width, height}`, or `{}`.
async fn image_size(ctx: &HubCtx, path: &Path) -> Value {
    let p = path.to_string_lossy().into_owned();
    if p.to_ascii_lowercase().ends_with(".svg") {
        let head: String = std::fs::read_to_string(path).map(|t| t.chars().take(2000).collect()).unwrap_or_default();
        let Some(start) = head.to_ascii_lowercase().find("<svg") else { return json!({"width": 0, "height": 0}) };
        let tag = &head[start..head[start..].find('>').map(|e| start + e + 1).unwrap_or(head.len())];
        let num = |k: &str| -> f64 {
            tag.find(&format!(" {k}=\""))
                .or_else(|| tag.find(&format!("\t{k}=\"")).or_else(|| tag.find(&format!("\n{k}=\""))))
                .and_then(|i| {
                    let v = &tag[i + k.len() + 3..];
                    let v: String = v.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
                    v.parse().ok()
                })
                .unwrap_or(0.0)
        };
        let vb: Vec<f64> = tag.find("viewBox=\"").map(|i| tag[i + 9..].split('"').next().unwrap_or("").split_whitespace().filter_map(|x| x.parse().ok()).collect()).unwrap_or_default();
        let (w, h) = (num("width"), num("height"));
        let pick = |a: f64, k: usize| if a != 0.0 { a } else { vb.get(k).copied().unwrap_or(0.0) };
        return json!({"width": pick(w, 2), "height": pick(h, 3)});
    }
    match exec(ctx, command("sips", &["-g", "pixelWidth", "-g", "pixelHeight", &p])).await {
        Ok(o) => json!({"width": sips_value(&o, "pixelWidth"), "height": sips_value(&o, "pixelHeight")}),
        Err(_) => json!({}),
    }
}

fn sips_value(out: &str, key: &str) -> u64 {
    out.find(&format!("{key}: ")).map(|i| out[i + key.len() + 2..].chars().take_while(char::is_ascii_digit).collect::<String>()).and_then(|d| d.parse().ok()).unwrap_or(0)
}

/// files.mjs `isAnimated(path, ext)`.
fn is_animated(path: &Path, ext: &str) -> bool {
    if ext == "gif" {
        return true;
    }
    let head = std::fs::read(path).map(|b| b.into_iter().take(4096).collect::<Vec<u8>>()).unwrap_or_default();
    let has = |needle: &[u8]| head.windows(needle.len()).any(|w| w == needle);
    match ext {
        "webp" => has(b"ANMF") || has(b"ANIM"),
        "png" => has(b"acTL"),
        _ => false,
    }
}

fn size_of(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// One chosen file staged under generated/attachments.
async fn stage(ctx: &HubCtx, att: &Path, src: &str) -> Result<Value, String> {
    let name = src.rsplit('/').next().unwrap_or(src).to_owned();
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    let image = ["png", "jpg", "jpeg", "gif", "heic", "webp", "tif", "tiff", "bmp"].contains(&ext.as_str());
    let video = ["mov", "mp4", "m4v", "webm", "avi", "mkv"].contains(&ext.as_str());
    let keep = ["png", "jpg", "jpeg", "gif", "webp"].contains(&ext.as_str());
    let id = format!("{}-{}", now36(), random36(4));
    let out = att.join(if image { format!("{id}.{}", if keep { ext.as_str() } else { "png" }) } else { format!("{id}-{name}") });
    let out_s = out.to_string_lossy().into_owned();
    if image && !keep {
        run_ok(ctx, "sips", &["-s", "format", "png", src, "--out", &out_s], 20000).await;
    } else {
        std::fs::copy(src, &out).map_err(|e| e.to_string())?;
    }
    let animated = image && is_animated(&out, &ext);
    let (mut thumb, mut feed) = (String::new(), String::new());
    if image {
        let fext = if keep && ext != "gif" { ext.clone() } else { "png".into() };
        let t = att.join(format!("{id}.thumb.png"));
        let f = att.join(format!("{id}.feed.{fext}"));
        let (t_s, f_s) = (t.to_string_lossy().into_owned(), f.to_string_lossy().into_owned());
        let feed_args: Vec<&str> = if fext == "png" { vec!["-s", "format", "png", "-Z", "1120", &out_s, "--out", &f_s] } else { vec!["-Z", "1120", &out_s, "--out", &f_s] };
        let magick_in = format!("{out_s}[0]");
        let thumb_args = ["-s", "format", "png", "-Z", "256", out_s.as_str(), "--out", t_s.as_str()];
        let (ok_t, ok_f) = tokio::join!(run_ok(ctx, "sips", &thumb_args, 20000), async {
            let magick_args = [magick_in.as_str(), f_s.as_str()];
            run_ok(ctx, "sips", &feed_args, 20000).await || run_ok(ctx, "magick", &magick_args, 20000).await
        });
        if ok_t && t.exists() {
            thumb = t_s;
        }
        if ok_f && f.exists() && (animated || size_of(&f) < size_of(&out)) {
            feed = f_s;
        } else if f.exists() {
            let _ = std::fs::remove_file(&f);
        }
    } else if video {
        let png = format!("{out_s}.png");
        if run_ok(ctx, "qlmanage", &["-t", "-s", "256", "-o", &att.to_string_lossy(), &out_s], 15000).await && Path::new(&png).exists() {
            thumb = png;
        }
    }
    let dims = if image {
        image_size(ctx, &out).await
    } else if video && !thumb.is_empty() {
        image_size(ctx, Path::new(&thumb)).await
    } else {
        json!({})
    };
    let mut o = serde_json::Map::new();
    o.insert("uri".into(), json!(format!("file://{out_s}")));
    o.insert("thumb".into(), json!(if thumb.is_empty() { String::new() } else { format!("file://{thumb}") }));
    o.insert("feed".into(), json!(if feed.is_empty() { String::new() } else { format!("file://{feed}") }));
    o.insert("name".into(), json!(name));
    o.insert("ext".into(), json!(ext));
    o.insert(
        "kind".into(),
        json!(if image {
            "image"
        } else if video {
            "video"
        } else {
            "file"
        }),
    );
    o.insert("size".into(), json!(if out.exists() { size_of(&out) } else { 0 }));
    if animated {
        o.insert("animated".into(), json!(true));
    }
    if let Some(d) = dims.as_object() {
        o.extend(d.clone());
    }
    Ok(Value::Object(o))
}

async fn attach_pick(ctx: Arc<HubCtx>, _req: HubRequest) -> HubResponse {
    let script = "set fs to choose file with prompt \"Attach photos & files\" with multiple selections allowed\nset out to \"\"\nrepeat with f in fs\nset out to out & POSIX path of f & linefeed\nend repeat\nreturn out";
    let cancelled = || answer(200, out(&json!({"ok": false, "cancelled": true, "files": []})), true);
    let Ok(list) = osa(&ctx, script).await else { return cancelled() };
    let att = ctx.store.gen_dir().join("attachments");
    let picks: Vec<&str> = list.split('\n').filter(|l| !l.is_empty()).collect();
    let staged = futures_join(picks.iter().map(|src| stage(&ctx, &att, src))).await;
    match staged.into_iter().collect::<Result<Vec<Value>, String>>() {
        Ok(files) => ok(&json!({"ok": true, "files": files})),
        Err(_) => cancelled(),
    }
}

/// Every future at once, their answers in order (`Promise.all`).
async fn futures_join<F: std::future::Future>(fs: impl Iterator<Item = F>) -> Vec<F::Output> {
    let fs: Vec<std::pin::Pin<Box<F>>> = fs.map(Box::pin).collect();
    let mut outs: Vec<Option<F::Output>> = (0..fs.len()).map(|_| None).collect();
    let mut fs: Vec<Option<std::pin::Pin<Box<F>>>> = fs.into_iter().map(Some).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (i, slot) in fs.iter_mut().enumerate() {
            if let Some(f) = slot {
                match f.as_mut().poll(cx) {
                    std::task::Poll::Ready(v) => {
                        outs[i] = Some(v);
                        *slot = None;
                    }
                    std::task::Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    })
    .await;
    outs.into_iter().map(|o| o.expect("each future ran to its end")).collect()
}

async fn avatar_image(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let kind = req.url["/avatar/image/".len()..].to_owned();
    let out_path = ctx.store.gen_dir().join("avatars").join(format!("{}.png", now36()));
    let out_s = out_path.to_string_lossy().into_owned();
    let name = match kind.as_str() {
        "gen" => {
            let b = match parse(&req.body) {
                Ok(b) => b,
                Err(e) => return refused(&e, true),
            };
            let desc = match trimmed(&b, "description") {
                Ok(Some(d)) if !d.is_empty() => d,
                Ok(_) => return refused("describe the avatar", true),
                Err(e) => return refused(&e, true),
            };
            let Some(gen) = ctx.ext.get::<AvatarImage>().cloned() else { return not_ported() };
            if let Err(e) = (gen.0)(ctx.clone(), desc.clone(), out_path.clone()).await {
                return refused(&e, true);
            }
            desc
        }
        "pick" => {
            let picked = osa(&ctx, "POSIX path of (choose file with prompt \"Choose an avatar image\" of type {\"public.image\"})").await.unwrap_or_default();
            if picked.is_empty() {
                return answer(200, out(&json!({"ok": false, "cancelled": true})), true);
            }
            if let Err(e) = exec(&ctx, command("sips", &["-s", "format", "png", &picked, "--out", &out_s])).await {
                return refused(&e, true);
            }
            picked.rsplit('/').next().unwrap_or(&picked).to_owned()
        }
        "paste" => {
            let script = format!("set f to open for access POSIX file \"{out_s}\" with write permission\n             write (the clipboard as «class PNGf») to f\n             close access f");
            if let Err(e) = osa(&ctx, &script).await {
                return refused(&e, true);
            }
            "Pasted image".to_owned()
        }
        other => return refused(&format!("unknown avatar source {other}"), true),
    };
    let dims = match exec(&ctx, command("sips", &["-g", "pixelWidth", "-g", "pixelHeight", &out_s])).await {
        Ok(d) => d,
        Err(e) => return refused(&e, true),
    };
    ok(&json!({"ok": true, "uri": format!("file://{out_s}"), "name": name, "w": sips_value(&dims, "pixelWidth"), "h": sips_value(&dims, "pixelHeight")}))
}

const SHAPES: [&str; 13] = ["blob", "cloud", "square", "sparkle", "clover", "heart", "flower", "teardrop", "tablet", "wedge", "house", "star", "hex"];
const COLORS: [&str; 11] = ["#FFFFFF", "#936439", "#FF263C", "#FF6700", "#FF9800", "#00C972", "#00BCA6", "#1084FE", "#9159FE", "#FF309B", "#777777"];

/// avatar-design.mjs `pickKitAvatar(description)`: the old /avatar/gen
/// choice, a kit shape and a swatch (blob, #1084FE when the model's answer
/// is off the lists). `Ok(None)`: no model client (501).
async fn pick_kit(ctx: &HubCtx, desc: &str) -> Result<Option<(String, String)>, String> {
    let (mut shape, mut color) = ("blob".to_owned(), "#1084FE".to_owned());
    let provider = std::env::var("GEN_PROVIDER").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| "anthropic".into());
    if provider == "anthropic" {
        let Some(model) = ctx.model.clone() else { return Ok(None) };
        let body = json!({
            "model": super::chat::model(),
            "max_tokens": 120,
            "system": format!("Pick an avatar shape and color for a blob character from a description. Reply ONLY with JSON {{\"shape\":<one of {}>,\"color\":<one of {}>}}.", SHAPES.join("|"), COLORS.join("|")),
            "messages": [{"role": "user", "content": desc}],
        });
        let m = model.create(&TurnCtx::default(), body).await?;
        let txt = text_of(&m);
        let found = match (txt.find('{'), txt.rfind('}')) {
            (Some(a), Some(z)) if z >= a => txt[a..=z].to_owned(),
            _ => "{}".to_owned(),
        };
        let j: Value = serde_json::from_str(&found).map_err(|e| format!("{} in JSON", e.to_string().split(" at line ").next().unwrap_or_default()))?;
        if let Some(s) = j.get("shape").and_then(Value::as_str).filter(|s| SHAPES.contains(s)) {
            shape = s.into();
        }
        if let Some(c) = j.get("color").and_then(Value::as_str).filter(|c| COLORS.contains(c)) {
            color = c.into();
        }
    }
    Ok(Some((shape, color)))
}

async fn avatar_gen(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let b = match parse(&req.body) {
        Ok(b) => b,
        Err(e) => return refused(&e, false),
    };
    let desc = match trimmed(&b, "description") {
        Ok(Some(d)) if !d.is_empty() => d,
        Ok(_) => return refused("describe the avatar", false),
        Err(e) => return refused(&e, false),
    };
    match pick_kit(&ctx, &desc).await {
        Ok(Some((shape, color))) => ok(&json!({"ok": true, "shape": shape, "color": color})),
        Ok(None) => not_ported(),
        Err(e) => refused(&e, false),
    }
}

fn text_of(m: &Value) -> String {
    m.get("content")
        .and_then(Value::as_array)
        .map(|c| c.iter().filter(|x| x.get("type") == Some(&json!("text"))).map(|x| js::string_of(x.get("text").unwrap_or(&Value::Null))).collect())
        .unwrap_or_default()
}

/// Design's model (config.mjs `AVATAR_MODEL`): `GEN_AVATAR_MODEL`, else
/// claude-sonnet-5 on Anthropic and the provider's model elsewhere.
fn avatar_model(settings: &Settings) -> String {
    std::env::var("GEN_AVATAR_MODEL").ok().filter(|m| !m.is_empty()).unwrap_or_else(|| if settings.anthropic() { "claude-sonnet-5".into() } else { settings.model.clone() })
}

/// `/\/(\.|%2e){2}(\/|$)/i`: a `..` segment as sent.
fn dot_dot(uri: &str) -> bool {
    let l = uri.to_ascii_lowercase();
    let b = l.as_bytes();
    let unit = |i: usize| -> Option<usize> {
        if b.get(i) == Some(&b'.') {
            Some(i + 1)
        } else if l[i.min(l.len())..].starts_with("%2e") {
            Some(i + 3)
        } else {
            None
        }
    };
    (0..b.len()).any(|i| b[i] == b'/' && unit(i + 1).and_then(unit).is_some_and(|j| j == b.len() || b[j] == b'/'))
}

/// `path.relative(root, p)` names a path inside `root`.
fn inside(root: &Path, p: &Path) -> bool {
    match p.strip_prefix(root) {
        Ok(rel) => {
            let r = rel.to_string_lossy();
            !r.is_empty() && !r.starts_with("..")
        }
        Err(_) => false,
    }
}

/// avatar-design.mjs `avatarImageBlock(uri)`: the image block for a
/// `file://` URI /avatar/image/pick or /paste returned; anything outside
/// the avatar folder is refused.
async fn avatar_image_block(ctx: &HubCtx, uri: &Value) -> Result<Value, String> {
    let refuse = || "image must be an avatar image from /avatar/image/pick or /avatar/image/paste".to_owned();
    let uri = uri.as_str().filter(|u| u.starts_with("file://") && !dot_dot(u)).ok_or_else(refuse)?;
    // fileURLToPath: a local file URL, no encoded "/"
    if uri.to_ascii_lowercase().contains("%2f") {
        return Err(refuse());
    }
    let path = url::Url::parse(uri).ok().and_then(|u| u.to_file_path().ok()).ok_or_else(refuse)?;
    let dir = ctx.store.gen_dir().join("avatars");
    if !inside(&dir, &path) {
        return Err(refuse());
    }
    let real = std::fs::canonicalize(&path).map_err(|_| "image not found".to_owned())?;
    let _ = std::fs::create_dir_all(&dir);
    // and where it really is: no symlink out
    let real_dir = std::fs::canonicalize(&dir).map_err(|e| e.to_string())?;
    if !inside(&real_dir, &real) || !real.is_file() {
        return Err(refuse());
    }
    let mut bytes = std::fs::read(&real).map_err(|e| e.to_string())?;
    // the Messages API takes up to 5 MB an image: a big photo goes down to 1024
    if bytes.len() > 3_500_000 {
        let small = dir.join(format!("design-{}.png", now36()));
        let (src, out_s) = (real.to_string_lossy().into_owned(), small.to_string_lossy().into_owned());
        exec(ctx, command("sips", &["-s", "format", "png", "-Z", "1024", &src, "--out", &out_s])).await?;
        bytes = std::fs::read(&small).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&small);
    }
    let media = if bytes.starts_with(&[0x89, 0x50]) {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF") {
        "image/gif"
    } else if bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        return Err("image is not a PNG, JPEG, GIF or WebP".into());
    };
    use base64::Engine as _;
    Ok(json!({"type": "image", "source": {"type": "base64", "media_type": media, "data": base64::engine::general_purpose::STANDARD.encode(&bytes)}}))
}

/// avatar-design.mjs `askDesigner(messages, model)`: the reply's text.
/// `chosen` is the picker's id ("gpt-5.5", "claude-haiku-5-5--fast"),
/// routed through opencodex as a bot's chosen model is; "" is Design's
/// default ([`avatar_model`]), never the chat's.
async fn ask_designer(ctx: Arc<HubCtx>, messages: Vec<Value>, chosen: String) -> Result<String, String> {
    let settings = Settings::global();
    if settings.anthropic() {
        let model = ctx.model.clone().ok_or_else(|| "no model client".to_owned())?;
        let body = json!({"model": avatar_model(settings), "max_tokens": 6000, "system": avatar_shape::design_system(), "messages": messages});
        let mut turn = TurnCtx::default();
        if !chosen.is_empty() {
            let fast = chosen.ends_with("--fast");
            let base = if fast { &chosen[..chosen.len() - "--fast".len()] } else { chosen.as_str() };
            turn.model = Some(ModelChoice { model: base.to_owned(), effort: String::new(), fast, name: None });
        }
        let m = model.create(&turn, body).await.map_err(|e| parse_llm(&e).1.to_owned())?;
        return Ok(text_of(&m));
    }
    let mut msgs = vec![json!({"role": "system", "content": avatar_shape::design_system()})];
    for m in &messages {
        let mut m = m.as_object().cloned().unwrap_or_default();
        let c = openai::oai_content(m.get("content"))?;
        m.insert("content".into(), c);
        msgs.push(Value::Object(m));
    }
    let body = json!({"model": if chosen.is_empty() { avatar_model(settings) } else { chosen }, "messages": msgs, "max_tokens": 6000});
    let (status, text) = openai::completions(settings, &body).await?;
    if !(200..300).contains(&status) {
        return Err(format!("llm-api {status}: {text}"));
    }
    let data = openai::json_of(&text)?;
    Ok(openai::choice(&data, &["message", "content"]).filter(|c| js::truthy(c)).map(js::string_of).unwrap_or_default())
}

/// `POST /avatar/design {prompt, image?, name?, model?}` → `{ok, candidates}`
/// (avatar-design.mjs `designAvatar`).
async fn avatar_design(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let b = match parse(&req.body) {
        Ok(b) => b,
        Err(e) => return refused(&e, true),
    };
    let prompt = match b.get("prompt").and_then(Value::as_str).map(|p| p.trim_matches(js::is_space)) {
        Some(p) if !p.is_empty() => p.to_owned(),
        _ => return refused("describe the bot or how it should look", true),
    };
    let block = match b.get("image").filter(|v| js::truthy(v)) {
        Some(img) => match avatar_image_block(&ctx, img).await {
            Ok(x) => Some(x),
            Err(e) => return refused(&e, true),
        },
        None => None,
    };
    let settings = Settings::global();
    if settings.mock {
        let c: Vec<Value> = avatar_shape::mock_candidates(block.is_some()).iter().map(avatar_shape::Candidate::json).collect();
        return ok(&json!({"ok": true, "candidates": c}));
    }
    if settings.anthropic() && ctx.model.is_none() {
        return not_ported();
    }
    let name = b.get("name").and_then(Value::as_str).map(|n| n.trim_matches(js::is_space).to_owned()).unwrap_or_default();
    let chosen = b.get("model").and_then(Value::as_str).map(|m| m.trim_matches(js::is_space).to_owned()).unwrap_or_default();
    let (ask_ctx, pick_ctx) = (ctx.clone(), ctx.clone());
    let r = avatar_shape::design_candidates(
        &prompt,
        &name,
        block,
        |messages| ask_designer(ask_ctx.clone(), messages, chosen.clone()),
        |p: String| {
            let ctx = pick_ctx.clone();
            // the fallback never fails the request
            async move { pick_kit(&ctx, &p).await.ok().flatten().map(|k| k.0).unwrap_or_else(|| "blob".into()) }
        },
    )
    .await;
    match r {
        Ok(c) => ok(&json!({"ok": true, "candidates": c.iter().map(avatar_shape::Candidate::json).collect::<Vec<_>>()})),
        Err(e) => refused(&e, true),
    }
}
