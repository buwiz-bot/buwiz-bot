//! The market's 12 routes (routes/market.mjs): `/plugins/catalog`,
//! `/plugins/detail`, `/plugins/icon/<key>`, `/plugins/skill`,
//! `/plugins/inspect` (2.1's review, no Node twin), `/plugins/install`,
//! `/plugins/uninstall`, `/plugins/settings`,
//! `/plugins/connect`, and the private skills' `POST /skills`,
//! `/skills/state`, `/skills/delete`.

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{HeaderValue, CACHE_CONTROL, CONTENT_TYPE};
use hyper::{Response, StatusCode};
use serde_json::{json, Map, Value};

use crate::hub::ctx::HubCtx;
use crate::hub::market::catalog::{cat_label, categories, encode_component, Market};
use crate::hub::market::install::{mcp_invalidate, mcp_status, plugin_inventory, remove_plugin_files, skill_file, upsert_connector};
use crate::hub::market::{at, entries, or, parse, resolve, review, skills, spread, string, truthy, unique};
use crate::hub::types::{json as answer, HubRequest, HubResponse, Pattern, Verb};
use crate::hub::Registry;
use crate::js;

pub fn register(routes: &mut Registry, market: Arc<Market>) {
    macro_rules! route {
        ($verb:expr, $pattern:expr, $f:ident) => {{
            let m = market.clone();
            routes.route($verb, $pattern, move |ctx, req| {
                let m = m.clone();
                async move { $f(&m, &ctx, req).await }
            });
        }};
    }
    route!(Verb::Get, Pattern::Prefix("/plugins/catalog"), catalog);
    route!(Verb::Post, Pattern::Exact("/plugins/inspect"), inspect);
    route!(Verb::Post, Pattern::Exact("/plugins/install"), install);
    route!(Verb::Post, Pattern::Exact("/plugins/uninstall"), uninstall);
    route!(Verb::Post, Pattern::Exact("/plugins/settings"), settings);
    route!(Verb::Get, Pattern::Prefix("/plugins/detail"), detail);
    route!(Verb::Post, Pattern::Exact("/plugins/connect"), connect);
    route!(Verb::Get, Pattern::Prefix("/plugins/icon/"), icon);
    route!(Verb::Get, Pattern::Prefix("/plugins/skill"), skill);
    routes.route(Verb::Post, Pattern::Exact("/skills"), |ctx, req| async move { skills_save(&ctx, req) });
    routes.route(Verb::Post, Pattern::Exact("/skills/state"), |ctx, req| async move { skills_state(&ctx, req) });
    routes.route(Verb::Post, Pattern::Exact("/skills/delete"), |ctx, req| async move { skills_delete(&ctx, req) });
}

/// market.mjs's `readBody`: the JSON body, `{}` when it isn't JSON.
fn body(req: &HubRequest) -> Value {
    let text = if req.body.is_empty() { "{}".as_bytes() } else { &req.body[..] };
    serde_json::from_slice(text).unwrap_or_else(|_| json!({}))
}

/// `String(v || "")`: how entryOf reads a key.
fn key_of(v: Option<&Value>) -> String {
    v.filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default()
}

fn ok(v: Value) -> HubResponse {
    answer(StatusCode::OK, &v)
}

/// `fail(res, e)`: 400 with the message's first 300 characters.
fn fail(e: String) -> HubResponse {
    answer(StatusCode::BAD_REQUEST, &json!({ "ok": false, "error": js::slice_utf16(&e, 300) }))
}

fn raw(status: StatusCode, content_type: &'static str, text: &'static str) -> HubResponse {
    let mut res = Response::new(Full::new(Bytes::from_static(text.as_bytes())).map_err(|never| match never {}).boxed());
    *res.status_mut() = status;
    res.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    res
}

/// `rows(...)` as JSON objects.
fn rows(ctx: &HubCtx, sql: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<Map<String, Value>>, String> {
    ctx.db.with(|db| {
        let mut st = db.prepare(sql)?;
        let names: Vec<String> = st.column_names().into_iter().map(str::to_owned).collect();
        let r = st.query_map(args, |r| {
            let mut m = Map::new();
            for (i, n) in names.iter().enumerate() {
                use rusqlite::types::ValueRef::*;
                let v = match r.get_ref(i)? {
                    Integer(x) => json!(x),
                    Real(x) => json!(x),
                    Text(t) => json!(String::from_utf8_lossy(t)),
                    Null | Blob(_) => Value::Null,
                };
                m.insert(n.clone(), v);
            }
            Ok(m)
        })?;
        r.collect()
    })
}

/// `GET /plugins/catalog[?rev=]`: the catalog rows and installed flags; a
/// matching rev answers `{rev, unchanged: true}`.
async fn catalog(m: &Market, ctx: &HubCtx, req: HubRequest) -> HubResponse {
    let installed = match rows(ctx, "SELECT name, meta FROM installed_plugins", &[]) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let meta_of = |key: &str| installed.iter().find(|r| r.get("name").and_then(Value::as_str) == Some(key)).map(|r| r.get("meta").cloned().unwrap_or(Value::Null));
    let mut keys: Vec<String> = installed.iter().map(|r| string(r.get("name"))).collect();
    keys.sort();
    let joined: String = installed.iter().map(|r| r.get("meta").filter(|v| !v.is_null()).map(js::string_of).unwrap_or_default()).collect();
    let rev = format!("{}|{}|{}", m.fetched_at, keys.join(","), if installed.is_empty() { 0 } else { joined.encode_utf16().count() });
    if req.query("rev").as_deref() == Some(rev.as_str()) {
        return ok(json!({ "rev": rev, "unchanged": true }));
    }
    let mut plugins = Vec::new();
    for e in &m.entries {
        let key = string(e.get("key"));
        let meta = meta_of(&key);
        if !(m.show_oauth_app || meta.is_some() || m.hidden_info(&key).is_none()) {
            continue;
        }
        let cats: Vec<Value> = entries(e.get("cats").filter(|c| js::truthy(c))).into_iter().map(|(_, c)| cat_label(&js::string_of(&c)).map_or(Value::Null, |l| json!(l))).collect();
        let mut keywords: Vec<Value> = entries(e.get("keywords").filter(|k| js::truthy(k))).into_iter().map(|(_, k)| k).collect();
        keywords.extend([e.get("name").cloned().unwrap_or(Value::Null), e.get("pluginName").cloned().unwrap_or(Value::Null)]);
        for list in ["connectors", "skills"] {
            keywords.extend(entries(e.get(list).filter(|l| js::truthy(l))).into_iter().map(|(_, c)| c.get("name").cloned().unwrap_or(Value::Null)));
        }
        plugins.push(json!({
            "name": key,
            "displayName": or(e.get("displayName"), e.get("name").cloned().unwrap_or(Value::Null)),
            "description": or(e.get("description"), json!("")),
            "category": category(e),
            "cats": cats,
            "homepage": or(e.get("websiteUrl"), Value::Null),
            "iconUrl": or(e.get("iconUrl"), Value::Null),
            "icon": m.icon_file(&key).map_or(Value::Null, |_| json!(format!("/plugins/icon/{}", encode_component(&key)))),
            "publisher": or(at(e.get("publisher"), "displayName"), or(at(e.get("publisher"), "name"), json!(""))),
            "keywords": keywords.into_iter().filter(js::truthy).collect::<Vec<_>>(),
            "domains": [],
            "private": truthy(e.get("marketplace")),
            "setup": !m.fields_of(ctx, &key).is_empty(),
            "unavailable": or(at(e.get("install"), "unavailable"), Value::Null),
            "installed": meta.is_some(),
            "meta": meta.map_or(Value::Null, |v| parse(v.as_str(), json!({}))),
        }));
    }
    ok(json!({ "marketplace": "grok-bot", "rev": rev, "categories": categories(), "plugins": plugins }))
}

/// `e.cats?.length ? catLabel[e.cats[0]] : e.category || "other"`
fn category(e: &Value) -> Value {
    match e.get("cats").and_then(Value::as_array).and_then(|c| c.first()) {
        Some(c) => cat_label(&js::string_of(c)).map_or(Value::Null, |l| json!(l)),
        None => or(e.get("category"), json!("other")),
    }
}

/// `POST /plugins/inspect {name}`: the plugin fetched into a staging
/// folder and its review (`{ok, staging, name, expires, setup, review}`);
/// nothing is installed.
async fn inspect(m: &Market, ctx: &HubCtx, req: HubRequest) -> HubResponse {
    let b = body(&req);
    let name = key_of(b.get("name").or(b.get("id")));
    match review::inspect(m, ctx, &name).await {
        Ok(v) => ok(v),
        Err(e) => fail(e),
    }
}

/// A refusal with a code the app reads (`{ok: false, code, error}`).
fn refuse(code: &str, e: &str) -> HubResponse {
    answer(StatusCode::BAD_REQUEST, &json!({ "ok": false, "code": code, "error": e }))
}

/// `POST /plugins/install {staging, approve: true, name?, values?}`: what
/// `/plugins/inspect` staged, after the person approved its review (D2).
/// A body without both is refused (`consent_required`); an unknown or
/// expired staging token is `review_expired`.
async fn install(m: &Market, ctx: &HubCtx, req: HubRequest) -> HubResponse {
    let b = body(&req);
    let token = b.get("staging").and_then(Value::as_str).unwrap_or("");
    if b.get("approve") != Some(&json!(true)) || token.is_empty() {
        return refuse("consent_required", "a plugin installs only after its review: POST /plugins/inspect, then /plugins/install with its staging token and approve: true");
    }
    let now = crate::hub::market::now(ctx);
    let staged = match review::open(m, token) {
        Some(s) if s.expired(now) => {
            let _ = std::fs::remove_dir_all(&s.dir);
            None
        }
        s => s,
    };
    let Some(staged) = staged else { return refuse("review_expired", "this review expired; review the plugin again") };
    let named = key_of(b.get("name"));
    if !named.is_empty() && m.entry_of(&named).map(|e| string(e.get("key"))) != Some(staged.key.clone()) {
        return refuse("invalid_args", &format!("the review is for {}, not {named}", staged.key));
    }
    match install_plugin(m, ctx, &b, staged).await {
        Ok(v) => ok(v),
        Err(e) => fail(e),
    }
}

async fn install_plugin(m: &Market, ctx: &HubCtx, b: &Value, staged: review::Staged) -> Result<Value, String> {
    let e = m.entry_of(&staged.key).ok_or_else(|| format!("unknown plugin {}", staged.key))?.clone();
    let key = string(e.get("key"));
    if truthy(b.get("values")) {
        let mut vals = m.settings_of(ctx, &key);
        for (k, v) in spread(b.get("values")) {
            vals.insert(k, v);
        }
        m.save_settings(ctx, &key, &vals)?;
    }
    let vals = m.settings_of(ctx, &key);
    let missing: Vec<String> = entries(e.get("fields").filter(|f| js::truthy(f)))
        .into_iter()
        .filter(|(_, f)| truthy(f.get("isRequired")) && !truthy(vals.get(&string(f.get("key")))))
        .map(|(_, f)| string(f.get("label")))
        .collect();
    // the staging folder stays: the same review installs once the values are in
    if !missing.is_empty() {
        return Err(format!("{} needs {} before it can be added", string(e.get("displayName")), missing.join(", ")));
    }
    let prev = m.installed_meta(ctx, &key);
    let meta = staged.meta.clone();
    staged.commit(m)?;
    let prev_list = |k: &str| or(at(prev.as_ref(), k), json!([]));
    // recorded first, so pluginRoot and materialize see the new files
    let mut first = spread(Some(&meta));
    first.insert("mcp".into(), prev_list("mcp"));
    first.insert("created".into(), prev_list("created"));
    let now = crate::hub::market::now(ctx);
    ctx.db.with(|db| {
        db.execute(
            "INSERT OR REPLACE INTO installed_plugins(name, meta, created_at) VALUES(?,?,COALESCE((SELECT created_at FROM installed_plugins WHERE name=?),?))",
            rusqlite::params![key, js::stringify(&Value::Object(first)), key, now],
        )
    })?;
    let (mut mcp, mut created, mut waiting) = (Vec::new(), entries(at(prev.as_ref(), "created").filter(|c| js::truthy(c))).into_iter().map(|(_, v)| v).collect::<Vec<_>>(), Vec::new());
    for server in m.server_configs(ctx, &key).keys().cloned().collect::<Vec<_>>() {
        let Some(mat) = m.materialize(ctx, &key, &server, None) else { continue };
        if !mat.runnable() {
            continue;
        }
        if !mat.missing.is_empty() {
            waiting.push(json!(server));
            continue;
        }
        let r = upsert_connector(m, ctx, &key, &server, None).await?;
        mcp.push(json!(r.id));
        if r.created {
            created.push(json!(r.id));
        }
    }
    let root = m.plugin_root(ctx, &key);
    let mut kinds = Vec::new();
    for k in ["skills", "commands", "agents", "hooks/hooks.json", ".lsp.json", "settings.json"] {
        if root.as_ref().is_some_and(|r| r.join(k).exists()) {
            kinds.push(json!(k));
        }
    }
    let mut full = spread(Some(&meta));
    let all_mcp = unique(entries(at(prev.as_ref(), "mcp").filter(|c| js::truthy(c))).into_iter().map(|(_, v)| v).chain(mcp));
    full.insert("mcp".into(), Value::Array(all_mcp.clone()));
    full.insert("created".into(), Value::Array(unique(created)));
    full.insert("kinds".into(), Value::Array(kinds.clone()));
    ctx.db.with(|db| db.execute("UPDATE installed_plugins SET meta=? WHERE name=?", rusqlite::params![js::stringify(&Value::Object(full)), key]))?;
    Ok(json!({ "ok": true, "name": key, "mcp": all_mcp, "kinds": kinds, "waiting": waiting }))
}

/// `POST /plugins/uninstall {name}`: the record, its files, and the
/// connector rows its install made (rows that predate it stay).
async fn uninstall(m: &Market, ctx: &HubCtx, req: HubRequest) -> HubResponse {
    let b = body(&req);
    let n = b.get("name").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
    let meta = m.installed_meta(ctx, &n);
    for (_, id) in entries(at(meta.as_ref(), "created").filter(|c| js::truthy(c))) {
        let id = js::string_of(&id);
        mcp_invalidate(ctx, &id).await;
        if let Err(e) = ctx.db.with(|db| db.execute("DELETE FROM mcp_servers WHERE id=?", [&id])) {
            return fail(e);
        }
    }
    let gone = ctx.db.with(|db| {
        db.execute("DELETE FROM installed_plugins WHERE name=?", [&n])?;
        db.execute("DELETE FROM plugin_settings WHERE plugin=?", [&n])
    });
    if let Err(e) = gone {
        return fail(e);
    }
    remove_plugin_files(m, &n);
    ok(json!({ "ok": true }))
}

/// `POST /plugins/settings {plugin, values, clear?}`.
async fn settings(m: &Market, ctx: &HubCtx, req: HubRequest) -> HubResponse {
    match save_settings(m, ctx, &body(&req)).await {
        Ok(()) => ok(json!({ "ok": true })),
        Err(e) => fail(e),
    }
}

async fn save_settings(m: &Market, ctx: &HubCtx, b: &Value) -> Result<(), String> {
    let key = string(m.entry_of(&key_of(b.get("plugin"))).ok_or_else(|| format!("unknown plugin {}", string(b.get("plugin"))))?.get("key"));
    let mut next = m.settings_of(ctx, &key);
    let values = b.get("values").filter(|v| js::truthy(v)).cloned().unwrap_or(json!({}));
    for f in m.fields_of(ctx, &key) {
        let fk = string(f.get("key"));
        let Some(v) = values.as_object().and_then(|o| o.get(&fk)) else { continue };
        let v = if v.is_null() { String::new() } else { js::string_of(v) };
        let v = v.trim_matches(js::is_space);
        if !v.is_empty() {
            next.insert(fk, json!(v));
        } else if !truthy(f.get("secret")) {
            next.shift_remove(&fk);
        }
    }
    for (_, k) in entries(b.get("clear").filter(|c| js::truthy(c))) {
        next.shift_remove(&js::string_of(&k));
    }
    m.save_settings(ctx, &key, &next)?;
    // the shared Google client re-applies to every installed Google plugin
    let mut keys = vec![key.clone()];
    if m.plugin_uses_google(ctx, &key) {
        for r in rows(ctx, "SELECT name FROM installed_plugins", &[])? {
            let k = string(r.get("name"));
            if k != key && m.plugin_uses_google(ctx, &k) {
                keys.push(k);
            }
        }
    }
    for k in keys {
        reapply(m, ctx, &k).await?;
    }
    Ok(())
}

/// `reapply(key)`: the plugin's connector rows rebuilt from its values,
/// and a row added for each connector no longer waiting.
async fn reapply(m: &Market, ctx: &HubCtx, key: &str) -> Result<(), String> {
    let key = string(m.entry_of(key).ok_or_else(|| "Cannot read properties of null (reading 'key')".to_owned())?.get("key"));
    let Some(mut meta) = m.installed_meta(ctx, &key) else { return Ok(()) };
    for server in m.server_configs(ctx, &key).keys().cloned().collect::<Vec<_>>() {
        let mine: Vec<Value> = entries(at(Some(&meta), "mcp").filter(|c| js::truthy(c))).into_iter().map(|(_, v)| v).collect();
        let ids: Vec<String> = rows(ctx, "SELECT id FROM mcp_servers WHERE name=?", &[&server])?.into_iter().map(|r| string(r.get("id"))).filter(|id| mine.contains(&json!(id))).collect();
        if !ids.is_empty() {
            for id in ids {
                upsert_connector(m, ctx, &key, &server, Some(id)).await?;
            }
        } else if m.materialize(ctx, &key, &server, None).is_some_and(|x| x.missing.is_empty()) {
            let r = upsert_connector(m, ctx, &key, &server, None).await?;
            meta["mcp"] = Value::Array(unique(mine.into_iter().chain([json!(r.id)])));
            if r.created {
                let created = entries(at(Some(&meta), "created").filter(|c| js::truthy(c))).into_iter().map(|(_, v)| v);
                meta["created"] = Value::Array(unique(created.chain([json!(r.id)])));
            }
        }
    }
    ctx.db.with(|db| db.execute("UPDATE installed_plugins SET meta=? WHERE name=?", rusqlite::params![js::stringify(&meta), key])).map(|_| ())
}

/// `accountNote`: what's missing, or how a connected account signs in.
fn account_note(m: &Market, ctx: &HubCtx, key: &str, server: &str, row: &Map<String, Value>, st: &Value) -> String {
    let Some(mat) = m.materialize(ctx, key, server, None) else { return String::new() };
    if !mat.missing.is_empty() {
        return format!("Add {} in Setup to connect", mat.missing.join(", "));
    }
    let authed = truthy(parse(row.get("auth").and_then(Value::as_str), json!({})).get("tokens"));
    let has_header = spread(Some(&parse(row.get("headers").and_then(Value::as_str), json!({})))).keys().any(|k| k.to_lowercase() == "authorization");
    let connected = st.get("status") == Some(&json!("connected"));
    if connected && !authed && has_header && mat.own {
        return "Connected · read-only (app token)".into();
    }
    if connected && !authed && has_header {
        return "Connected · token".into();
    }
    String::new()
}

/// `GET /plugins/detail?name=`: the entry, what an installed copy ships,
/// each connector's accounts and status (an idle enabled one is probed in
/// the background), and the setup fields.
async fn detail(m: &Market, ctx: &HubCtx, req: HubRequest) -> HubResponse {
    let name = req.query("name").unwrap_or_default();
    let Some(e) = m.entry_of(&name).cloned() else { return answer(StatusCode::NOT_FOUND, &json!({ "error": "unknown plugin" })) };
    let key = string(e.get("key"));
    let meta = m.installed_meta(ctx, &key);
    let root = meta.as_ref().and_then(|_| m.plugin_root(ctx, &key));
    let inv = match &root {
        Some(root) => plugin_inventory(m, ctx, &key, root),
        None => json!({
            "skills": entries(e.get("skills").filter(|s| js::truthy(s))).into_iter().map(|(_, s)| {
                let mut o = Map::new();
                if let Some(n) = s.get("name") { o.insert("name".into(), n.clone()); }
                o.insert("description".into(), or(s.get("description"), json!("")));
                o.insert("source".into(), if resolve::source_url_info(s.get("sourceUrl")).is_some() { s["sourceUrl"].clone() } else { Value::Null });
                Value::Object(o)
            }).collect::<Vec<_>>(),
            "commands": [], "agents": [], "hooks": false, "lsp": false, "servers": Value::Object(m.server_configs(ctx, &key)), "manifest": {},
        }),
    };
    let servers = spread(inv.get("servers"));
    let mut names: Vec<String> = Vec::new();
    for n in entries(e.get("connectors").filter(|c| js::truthy(c))).into_iter().map(|(_, c)| string(c.get("name"))).chain(servers.keys().cloned()) {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    let vals = m.settings_of(ctx, &key);
    let mut mcp = Vec::new();
    for name in names {
        let cfg = servers.get(&name);
        let list = match rows(ctx, "SELECT * FROM mcp_servers WHERE name=? ORDER BY created_at", &[&name]) {
            Ok(r) => r,
            Err(e) => return fail(e),
        };
        let mat = cfg.and_then(|c| m.materialize(ctx, &key, &name, Some(c)));
        let missing = mat.as_ref().is_some_and(|x| !x.missing.is_empty());
        let status_of = |row: &Map<String, Value>, st: &Value| -> Value {
            if !truthy(row.get("enabled")) {
                json!("off")
            } else if missing && st.get("status") != Some(&json!("connected")) {
                json!("needs-auth")
            } else {
                st.get("status").cloned().unwrap_or(Value::Null)
            }
        };
        let mut accounts = Vec::new();
        for (i, r) in list.iter().enumerate() {
            let id = string(r.get("id"));
            let st = mcp_status(ctx, &id);
            if truthy(r.get("enabled")) && st.get("status") == Some(&json!("idle")) && !missing {
                if let Some(host) = ctx.mcp.clone() {
                    tokio::spawn(async move {
                        host.ensure(&id).await;
                    });
                }
            }
            accounts.push(json!({
                "id": r.get("id").cloned().unwrap_or(Value::Null),
                "label": or(r.get("account"), if i > 0 { r.get("id").cloned().unwrap_or(Value::Null) } else { json!("default") }),
                "status": status_of(r, &st),
                "tools": st.get("tools").cloned().unwrap_or(Value::Null),
                "err": or(st.get("err"), Value::Null),
                "authed": truthy(parse(r.get("auth").and_then(Value::as_str), json!({})).get("tokens")),
                "note": if meta.is_some() && cfg.is_some() { account_note(m, ctx, &key, &name, r, &st) } else { String::new() },
            }));
        }
        let row = list.first();
        let st = row.map_or_else(|| json!({ "status": "off", "tools": 0 }), |r| mcp_status(ctx, &string(r.get("id"))));
        let transport = match cfg {
            Some(c) if truthy(c.get("url")) => "remote",
            Some(_) => "command",
            None => "unknown",
        };
        let callback = match &mat {
            Some(x) if x.own => {
                let id = row.and_then(|r| r.get("id")).filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_else(|| m.server_id_for(ctx, &key, &name));
                json!(crate::helper::oauth::fixed_redirect_url(m.port, &id))
            }
            _ => Value::Null,
        };
        mcp.push(json!({
            "name": name,
            "transport": transport,
            "id": row.and_then(|r| r.get("id")).filter(|v| js::truthy(v)).cloned().unwrap_or(Value::Null),
            "status": row.map_or(json!("not-added"), |r| status_of(r, &st)),
            "tools": st.get("tools").cloned().unwrap_or(Value::Null),
            "err": or(st.get("err"), Value::Null),
            "accounts": accounts,
            "unavailable": if cfg.is_some() { Value::Null } else { json!("Its configuration isn't published in the plugin's repository") },
            "needs": mat.as_ref().map_or(json!([]), |x| json!(x.missing)),
            "callbackUrl": callback,
        }));
    }
    let fields: Vec<Value> = m
        .fields_of(ctx, &key)
        .into_iter()
        .map(|f| {
            let fk = string(f.get("key"));
            let mut o = spread(Some(&f));
            o.insert("set".into(), json!(truthy(vals.get(&fk))));
            o.insert("value".into(), if truthy(f.get("secret")) { json!("") } else { or(vals.get(&fk), json!("")) });
            Value::Object(o)
        })
        .collect();
    let mut manifest = spread(inv.get("manifest"));
    manifest.insert("author".into(), or(manifest.get("author"), or(at(e.get("publisher"), "displayName"), Value::Null)));
    let marketplace = e.get("marketplace").filter(|v| js::truthy(v));
    ok(json!({
        "name": key,
        "displayName": or(e.get("displayName"), e.get("name").cloned().unwrap_or(Value::Null)),
        "description": or(e.get("description"), json!("")),
        "category": category(&e),
        "homepage": or(e.get("websiteUrl"), Value::Null),
        "source": or(e.get("repositoryUrl"), or(at(e.get("install"), "repo"), Value::Null)),
        "iconUrl": or(e.get("iconUrl"), Value::Null),
        "icon": m.icon_file(&key).map_or(Value::Null, |_| json!(format!("/plugins/icon/{}", encode_component(&key)))),
        "needsOAuthApp": m.hidden_info(&key).unwrap_or(Value::Null),
        "publisher": or(at(e.get("publisher"), "displayName"), or(at(e.get("publisher"), "name"), json!(""))),
        "availability": match marketplace {
            Some(mk) => json!(format!("Private marketplace ({})", js::string_of(&or(mk.get("displayName"), mk.get("name").cloned().unwrap_or(Value::Null))))),
            None => json!("Public"),
        },
        "unavailable": or(at(e.get("install"), "unavailable"), Value::Null),
        "installed": meta.is_some(),
        "meta": meta.clone().unwrap_or(Value::Null),
        "skills": inv.get("skills").cloned().unwrap_or(Value::Null),
        "commands": inv.get("commands").cloned().unwrap_or(Value::Null),
        "agents": inv.get("agents").cloned().unwrap_or(Value::Null),
        "hooks": inv.get("hooks").cloned().unwrap_or(Value::Null),
        "lsp": inv.get("lsp").cloned().unwrap_or(Value::Null),
        "manifest": manifest,
        "mcp": mcp,
        "fields": fields,
    }))
}

/// `POST /plugins/connect {plugin, server, label?, open?}`: one of a
/// plugin's connectors as an account, probed at once; a remote one still
/// signed out starts its OAuth.
async fn connect(m: &Market, ctx: &HubCtx, req: HubRequest) -> HubResponse {
    match connect_account(m, ctx, &body(&req)).await {
        Ok(v) => ok(v),
        Err(e) => fail(e),
    }
}

async fn connect_account(m: &Market, ctx: &HubCtx, b: &Value) -> Result<Value, String> {
    let (plugin, server) = (string(b.get("plugin")), string(b.get("server")));
    let key = match m.entry_of(&key_of(b.get("plugin"))) {
        Some(e) if m.installed_row(ctx, &string(e.get("key"))).is_some() => string(e.get("key")),
        _ => return Err(format!("plugin {plugin} is not installed")),
    };
    let mat = m.materialize(ctx, &key, &server, None).filter(|x| x.runnable()).ok_or_else(|| format!("{plugin} declares no server {server}"))?;
    if !mat.missing.is_empty() {
        return Err(format!("Add {} in Setup first", mat.missing.join(", ")));
    }
    let label = b.get("label").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
    let lab = js::slice_utf16(label.trim_matches(js::is_space), 40);
    if ["grok", "default"].contains(&lab.to_lowercase().as_str()) {
        return Err(format!("{lab} is a reserved account label"));
    }
    let list = rows(ctx, "SELECT * FROM mcp_servers WHERE name=? ORDER BY created_at", &[&server])?;
    if !lab.is_empty() && list.iter().any(|r| r.get("account").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default().to_lowercase() == lab.to_lowercase()) {
        return Err(format!("an account labeled {lab} already exists"));
    }
    let mut id = if lab.is_empty() { list.first().and_then(|r| r.get("id")).filter(|v| js::truthy(v)).map(js::string_of) } else { None };
    if !lab.is_empty() {
        let base: String = crate::hub::market::dashes(&format!("{server}-{lab}"), "-").trim_matches('-').chars().take(24).collect();
        let base = if base.is_empty() { "srv".to_owned() } else { base };
        let mut candidate = base.clone();
        let mut i = 2;
        while !rows(ctx, "SELECT id FROM mcp_servers WHERE id=?", &[&candidate])?.is_empty() {
            candidate = format!("{base}-{i}");
            i += 1;
        }
        id = Some(candidate);
    }
    let r = upsert_connector(m, ctx, &key, &server, id).await?;
    ctx.db.with(|db| {
        if !lab.is_empty() {
            db.execute("UPDATE mcp_servers SET account=? WHERE id=?", rusqlite::params![lab, r.id])?;
        }
        db.execute("UPDATE mcp_servers SET enabled=1, agent_pending=NULL WHERE id=?", [&r.id])
    })?;
    let mut meta = m.installed_meta(ctx, &key).unwrap_or(json!({}));
    let list_of = |meta: &Value, k: &str| entries(at(Some(meta), k).filter(|c| js::truthy(c))).into_iter().map(|(_, v)| v).collect::<Vec<_>>();
    meta["mcp"] = Value::Array(unique(list_of(&meta, "mcp").into_iter().chain([json!(r.id)])));
    if r.created {
        meta["created"] = Value::Array(unique(list_of(&meta, "created").into_iter().chain([json!(r.id)])));
    }
    ctx.db.with(|db| db.execute("UPDATE installed_plugins SET meta=? WHERE name=?", rusqlite::params![js::stringify(&meta), key]))?;
    let row_url = rows(ctx, "SELECT url FROM mcp_servers WHERE id=?", &[&r.id])?.first().and_then(|r| r.get("url").cloned()).unwrap_or(Value::Null);
    let has_header = mat.headers.keys().any(|k| k.to_lowercase() == "authorization");
    mcp_invalidate(ctx, &r.id).await;
    let rec = match &ctx.mcp {
        Some(host) => host.ensure(&r.id).await,
        None => json!({ "ok": false, "err": "no MCP host" }),
    };
    let needs = rec.get("needsAuth").is_some_and(js::truthy) || (rec.get("ok") == Some(&json!(false)) && at(rec.get("status"), "status") == Some(&json!("needs-auth")));
    let live = rec.get("ok") == Some(&json!(true)) && !rec.get("needsAuth").is_some_and(js::truthy);
    let mut auth = json!({ "ok": true, "status": if live { "connected" } else if needs { "needs-auth" } else { "error" }, "err": or(rec.get("err"), Value::Null), "token": has_header });
    if js::truthy(&row_url) && !live && (!mat.oauth_client.is_null() || !has_header) {
        if let Some(host) = &ctx.mcp {
            auth = host.auth_begin(&r.id, b.get("open") != Some(&json!(false))).await;
        }
    }
    let mut out = spread(Some(&auth));
    out.insert("ok".into(), json!(true));
    out.insert("id".into(), json!(r.id));
    Ok(Value::Object(out))
}

/// `GET /plugins/icon/<key>`: the logo file; 404 with no body.
async fn icon(m: &Market, _ctx: &HubCtx, req: HubRequest) -> HubResponse {
    let rest = &req.url["/plugins/icon/".len()..];
    let key = js::decode_uri_component(rest.split('?').next().unwrap_or("")).unwrap_or_default();
    let Some(f) = m.icon_file(&key) else {
        let mut res = Response::new(Full::new(Bytes::new()).map_err(|never| match never {}).boxed());
        *res.status_mut() = StatusCode::NOT_FOUND;
        return res;
    };
    let Ok(bytes) = std::fs::read(&f) else {
        let mut res = Response::new(Full::new(Bytes::new()).map_err(|never| match never {}).boxed());
        *res.status_mut() = StatusCode::NOT_FOUND;
        return res;
    };
    let ext = f.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let ct = match ext.as_str() {
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    };
    let mut res = Response::new(Full::new(Bytes::from(bytes)).map_err(|never| match never {}).boxed());
    res.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static(ct));
    res.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("max-age=86400"));
    res
}

/// `GET /plugins/skill?plugin=&skill=`: an installed plugin's SKILL.md,
/// else the catalog's description and link.
async fn skill(m: &Market, ctx: &HubCtx, req: HubRequest) -> HubResponse {
    let e = m.entry_of(&req.query("plugin").unwrap_or_default()).cloned();
    let skill = req.query("skill").unwrap_or_default();
    let file = e.as_ref().and_then(|e| {
        let key = string(e.get("key"));
        m.installed_row(ctx, &key).and_then(|_| skill_file(m, ctx, &key, &skill))
    });
    match file.filter(|f| f.exists()) {
        Some(f) => {
            let body = std::fs::read_to_string(&f).unwrap_or_default();
            let description = crate::hub::market::install::front_desc(&body);
            ok(json!({ "name": skill, "body": body, "description": description }))
        }
        None => {
            let c = e.as_ref().and_then(|e| entries(e.get("skills").filter(|s| js::truthy(s))).into_iter().map(|(_, s)| s).find(|s| s.get("name") == Some(&json!(skill))));
            match c {
                None => answer(StatusCode::NOT_FOUND, &json!({ "error": "no such skill" })),
                Some(c) => ok(json!({ "name": skill, "description": or(c.get("description"), json!("")), "body": "", "source": or(c.get("sourceUrl"), Value::Null) })),
            }
        }
    }
}

/// `POST /skills`.
fn skills_save(ctx: &HubCtx, req: HubRequest) -> HubResponse {
    let text = if req.body.is_empty() { "{}".as_bytes() } else { &req.body[..] };
    let parsed: Result<Value, String> = serde_json::from_slice(text).map_err(|e| e.to_string());
    let result = parsed.and_then(|b| if b.is_null() { Err("Cannot read properties of null (reading 'id')".into()) } else { skills::save(ctx, &b) });
    match result {
        Ok(id) => ok(json!({ "ok": true, "id": id })),
        Err(e) => answer(StatusCode::BAD_REQUEST, &json!({ "ok": false, "error": js::slice_utf16(&e, 300) })),
    }
}

/// `POST /skills/state`.
fn skills_state(ctx: &HubCtx, req: HubRequest) -> HubResponse {
    let text = if req.body.is_empty() { "{}".as_bytes() } else { &req.body[..] };
    match serde_json::from_slice::<Value>(text).ok().filter(|b| !b.is_null()).map(|b| skills::set_state(ctx, &b)) {
        Some(Ok(())) => raw(StatusCode::OK, "application/json", r#"{"ok":true}"#),
        _ => raw(StatusCode::BAD_REQUEST, "application/json", r#"{"ok":false}"#),
    }
}

/// `POST /skills/delete`. A body that isn't JSON (or is null) takes Node
/// down with no answer (its parse is outside any try); Rust answers 400
/// `{"ok":false}` instead (D8).
fn skills_delete(ctx: &HubCtx, req: HubRequest) -> HubResponse {
    let text = if req.body.is_empty() { "{}".as_bytes() } else { &req.body[..] };
    match serde_json::from_slice::<Value>(text).ok().filter(|b| !b.is_null()).map(|b| skills::delete(ctx, &b)) {
        Some(Ok(())) => raw(StatusCode::OK, "application/json", r#"{"ok":true}"#),
        _ => raw(StatusCode::BAD_REQUEST, "application/json", r#"{"ok":false}"#),
    }
}

#[cfg(test)]
mod tests {
    //! The plain plugin review (2.1 §2.2, D2) over a local git repository
    //! and a vendored plugin: no network.
    use super::*;
    use crate::hub::traits::Clock;
    use crate::hub::Hub;
    use hyper::header::HeaderMap;
    use hyper::Method;
    use rusqlite::OptionalExtension;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    struct Fixture {
        hub: Hub,
        root: PathBuf,
        plugins: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git").args(["-c", "user.email=t@example.com", "-c", "user.name=t", "-c", "init.defaultBranch=main"]).args(args).current_dir(dir).output().unwrap();
        assert!(ok.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&ok.stderr));
    }

    /// A catalog with `demo` (a local repo: two servers, a skill, a
    /// command, hooks, one required field) and the vendored `vend`.
    fn fixture(label: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!("rox-mkt-review-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        write(&repo.join(".claude-plugin/plugin.json"), r#"{"name":"demo","version":"1.2.0","author":{"name":"Ada"}}"#);
        write(
            &repo.join(".mcp.json"),
            r#"{"mcpServers":{"demo-cli":{"command":"npx","args":["-y","demo-mcp"],"env":{"DEMO_API_KEY":"${DEMO_API_KEY}","DEMO_REGION":"eu-literal-value"}},"demo-web":{"url":"https://mcp.demo.invalid/mcp"}}}"#,
        );
        write(&repo.join("skills/hello/SKILL.md"), "---\nname: hello\ndescription: Says hello.\n---\n");
        write(&repo.join("commands/run.md"), "---\ndescription: Runs.\n---\n");
        write(&repo.join("hooks/hooks.json"), "{}");
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "fixture"]);
        let server = root.join("server");
        let catalog = json!({ "plugins": [{
            "key": "demo", "name": "demo", "displayName": "Demo", "description": "A demo.", "publisher": { "displayName": "Demo Co" },
            "install": { "repo": format!("file://{}", repo.display()), "path": "" },
            "fields": [{ "key": "DEMO_API_KEY", "label": "Demo API Key", "isRequired": true, "isSecret": true }],
        }] });
        write(&server.join("plugins/grokbot-catalog.json"), &catalog.to_string());
        write(&server.join("plugins/marketplace.json"), r#"{"plugins":[{"name":"vend","description":"Vendored.","source":{"type":"local","path":"./external_plugins/vend"}}]}"#);
        write(&server.join("plugins/external_plugins/vend/skills/tidy/SKILL.md"), "---\ndescription: Tidies.\n---\n");
        let data = root.join("data");
        std::fs::create_dir_all(data.join("generated")).unwrap();
        let market = Arc::new(Market::load(&server, &data, 9));
        let ctx = HubCtx::new(data.clone(), Clock::manual());
        let hub = Hub::with(ctx, Vec::new(), |routes, _| register(routes, market));
        crate::hub::market::schema(hub.ctx()).unwrap();
        Fixture { hub, root, plugins: data.join("plugins") }
    }

    async fn post(f: &Fixture, url: &str, body: Value) -> (u16, Value) {
        let req = HubRequest { method: Method::POST, url: url.into(), headers: HeaderMap::new(), body: Bytes::from(body.to_string()), params: Vec::new() };
        let res = f.hub.answer(req).await;
        let status = res.status().as_u16();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    fn installed(f: &Fixture, key: &str) -> Option<Value> {
        let meta: Option<String> = f.hub.ctx().db.with(|db| db.query_row("SELECT meta FROM installed_plugins WHERE name=?", [key], |r| r.get(0)).optional()).unwrap();
        meta.map(|m| serde_json::from_str(&m).unwrap())
    }

    #[tokio::test]
    async fn mkt_review_install_without_an_approved_review_is_refused() {
        let f = fixture("refuse");
        for body in [json!({ "name": "demo" }), json!({ "name": "demo", "staging": "0".repeat(32) }), json!({ "name": "demo", "approve": true }), json!({ "name": "demo", "staging": "0".repeat(32), "approve": "yes" })] {
            let (status, v) = post(&f, "/plugins/install", body.clone()).await;
            assert_eq!((status, v["code"].as_str()), (400, Some("consent_required")), "{body}");
        }
        // a token no inspect made (or not a token at all)
        for token in ["0".repeat(32), "../demo".into()] {
            let (status, v) = post(&f, "/plugins/install", json!({ "staging": token, "approve": true })).await;
            assert_eq!((status, v["code"].as_str()), (400, Some("review_expired")));
        }
        assert!(!f.plugins.join("demo").exists() && installed(&f, "demo").is_none());
    }

    #[tokio::test]
    async fn mkt_review_inspect_stages_and_install_moves_it_into_place() {
        let f = fixture("flow");
        let (status, v) = post(&f, "/plugins/inspect", json!({ "name": "demo" })).await;
        assert_eq!(status, 200, "{v}");
        let token = v["staging"].as_str().unwrap().to_owned();
        assert_eq!((v["name"].as_str(), v["setup"].as_bool(), v["expires"].as_i64()), (Some("demo"), Some(true), Some(review::TTL_MS)));
        let r = &v["review"];
        assert_eq!((r["title"].as_str(), r["publisher"].as_str(), r["author"].as_str(), r["version"].as_str()), (Some("Demo"), Some("Demo Co"), Some("Ada"), Some("1.2.0")));
        assert_eq!((r["signed"].as_bool(), r["unsigned"].as_str()), (Some(false), Some("Unsigned: Buwiz can't check who made it")));
        assert_eq!(
            r["servers"],
            json!([
                { "name": "demo-cli", "transport": "command", "url": null, "command": "npx -y demo-mcp", "env": ["DEMO_API_KEY", "DEMO_REGION"], "headers": [], "auth": "none", "unavailable": null },
                { "name": "demo-web", "transport": "remote", "url": "https://mcp.demo.invalid/mcp", "command": null, "env": [], "headers": [], "auth": "oauth", "unavailable": null },
            ])
        );
        assert_eq!(r["skills"], json!([{ "name": "hello", "description": "Says hello." }]));
        assert_eq!(r["listed"], json!({ "commands": ["run"], "agents": [], "hooks": true, "lsp": false }));
        // env names, never values
        assert!(!v.to_string().contains("eu-literal-value"));
        // staged, not installed
        let staged = f.plugins.join(".staging").join(&token);
        assert!(staged.join("files/demo/.mcp.json").exists());
        assert!(!f.plugins.join("demo").exists() && installed(&f, "demo").is_none());

        // the required field is checked at install; the review stays
        let (status, v) = post(&f, "/plugins/install", json!({ "staging": token, "approve": true })).await;
        assert_eq!((status, v["error"].as_str()), (400, Some("Demo needs Demo API Key before it can be added")));
        assert!(staged.exists());
        // the review is for demo only
        let (_, v) = post(&f, "/plugins/install", json!({ "name": "vend", "staging": token, "approve": true })).await;
        assert_eq!(v["code"], "invalid_args");

        let (status, v) = post(&f, "/plugins/install", json!({ "name": "demo", "staging": token, "approve": true, "values": { "DEMO_API_KEY": "k-123" } })).await;
        assert_eq!(status, 200, "{v}");
        assert_eq!((v["ok"].as_bool(), v["name"].as_str()), (Some(true), Some("demo")));
        assert_eq!(v["mcp"], json!(["demo-cli", "demo-web"]));
        assert_eq!(v["kinds"], json!(["skills", "commands", "hooks/hooks.json"]));
        assert!(f.plugins.join("demo/.mcp.json").exists() && !staged.exists());
        let meta = installed(&f, "demo").unwrap();
        assert_eq!((meta["root"].as_str(), meta["created"].clone()), (Some("demo"), json!(["demo-cli", "demo-web"])));
        // a review installs once
        let (_, v) = post(&f, "/plugins/install", json!({ "staging": token, "approve": true })).await;
        assert_eq!(v["code"], "review_expired");
    }

    #[tokio::test]
    async fn mkt_review_staging_expires_after_30_minutes_and_on_the_next_inspect() {
        let f = fixture("expiry");
        let (_, first) = post(&f, "/plugins/inspect", json!({ "name": "demo" })).await;
        let first = first["staging"].as_str().unwrap().to_owned();
        // the next inspect of the plugin replaces the earlier review
        let (_, second) = post(&f, "/plugins/inspect", json!({ "name": "demo" })).await;
        let second = second["staging"].as_str().unwrap().to_owned();
        assert_ne!(first, second);
        assert!(!f.plugins.join(".staging").join(&first).exists());
        let (_, v) = post(&f, "/plugins/install", json!({ "staging": first, "approve": true, "values": { "DEMO_API_KEY": "k" } })).await;
        assert_eq!(v["code"], "review_expired");
        // 30 minutes on, it has expired: refused and deleted
        f.hub.ctx().clock.manual_step(Some((review::TTL_MS + 1) as f64), false);
        let (status, v) = post(&f, "/plugins/install", json!({ "staging": second, "approve": true, "values": { "DEMO_API_KEY": "k" } })).await;
        assert_eq!((status, v["code"].as_str()), (400, Some("review_expired")));
        assert!(!f.plugins.join(".staging").join(&second).exists());
        assert!(installed(&f, "demo").is_none());
        // an expired one of another plugin goes at the next inspect too
        let (_, v) = post(&f, "/plugins/inspect", json!({ "name": "vend" })).await;
        let vend = v["staging"].as_str().unwrap().to_owned();
        f.hub.ctx().clock.manual_step(Some((2 * review::TTL_MS + 2) as f64), false);
        post(&f, "/plugins/inspect", json!({ "name": "demo" })).await;
        assert!(!f.plugins.join(".staging").join(&vend).exists());
    }

    #[tokio::test]
    async fn mkt_review_a_vendored_plugin_reviews_and_installs_in_place() {
        let f = fixture("vend");
        let (status, v) = post(&f, "/plugins/inspect", json!({ "name": "vend" })).await;
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["review"]["skills"], json!([{ "name": "tidy", "description": "Tidies." }]));
        assert_eq!((v["review"]["servers"].clone(), v["review"]["unsigned"].as_str()), (json!([]), Some(review::UNSIGNED)));
        let (status, v) = post(&f, "/plugins/install", json!({ "staging": v["staging"], "approve": true })).await;
        assert_eq!((status, v["ok"].as_bool()), (200, Some(true)), "{v}");
        assert_eq!(installed(&f, "vend").unwrap()["path"], "./external_plugins/vend");
        assert!(!f.plugins.join("vend").exists());
        let (_, v) = post(&f, "/plugins/inspect", json!({ "name": "nope" })).await;
        assert_eq!(v["error"], "unknown plugin nope");
    }
}
