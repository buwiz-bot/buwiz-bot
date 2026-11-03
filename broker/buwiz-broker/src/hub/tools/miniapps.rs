//! The agent's mini app tools (drafts' install contract, part 2): Rust's
//! own, not in `defs.json` (which stays Node's, DEFS-01). `app_install`
//! fetches and verifies a GitHub release (`helper::release`), inspects it
//! and posts an `app_install` card in the chat: only the user's Install
//! installs it. `app_uninstall` posts an `app_uninstall` card the same way,
//! and `app_setup` an `app_setup` card whose fields the user changes a mini
//! app's setup in (the shared Google client; the model never sees a value);
//! `app_signin` posts the `account_required` Sign in card for an account
//! (with the setup it lacks), or says who is signed in already;
//! `app_enable` / `app_disable` act at once; `app_list` lists, with each
//! app's setup readiness and accounts (never a value or token). A
//! package's card also names its folder (`package`), skills and accounts.

use serde_json::{json, Value};

use super::{Out, Run};
use crate::helper::plugins::{parse, Host};
use crate::helper::release;
use crate::hub::ctx::HubCtx;
use crate::hub::routines::store_host::StoreHost;
use crate::hub::routines::threads;
use crate::js;

pub const NAMES: [&str; 7] = ["app_install", "app_list", "app_enable", "app_disable", "app_uninstall", "app_setup", "app_signin"];

/// The GitHub API base `app_install` reads (tests: a local server).
#[derive(Clone)]
pub struct GithubApi(pub String);

fn id_schema(what: &str) -> Value {
    json!({"type": "object", "properties": {"id": {"type": "string", "description": format!("The mini app's id (app_list shows them), e.g. app.buwiz.google-workspace; {what}")}}, "required": ["id"]})
}

/// The tools' schemas, as the model sees them.
pub fn defs() -> Vec<Value> {
    vec![
        json!({
            "name": "app_install",
            "description": "Offer to install a mini app from its GitHub release. Send its name (\"word\", \"Google Workspace\", optionally with a version: \"word@v1.0.0\"), which means https://github.com/buwiz-app/<name> (lowercased, spaces as -), or a repository URL (https://github.com/<owner>/<repo>, optionally /releases/tag/<tag>) or <owner>/<repo>, which is used as given. The release is downloaded and verified against the trusted publishers, then a card in this chat asks the user to review it and press Install. You cannot install it yourself, and you never ask for or pass its settings or secrets: the card asks the user for them.",
            "input_schema": {"type": "object", "properties": {"url": {"type": "string", "description": "a mini app's name (buwiz-app/<name>), https://github.com/<owner>/<repo>[/releases/tag/<tag>] or <owner>/<repo>"}}, "required": ["url"]},
        }),
        json!({
            "name": "app_list",
            "description": "List the installed mini apps: id, title, version, state, whether each is enabled, its setup (ready or missing, per provider) and its accounts (state and who is signed in). Read it to tell \"needs setup\" (app_setup or the sign-in card asks for it) from \"needs sign-in\" (app_signin) from \"connected\".",
            "input_schema": {"type": "object", "properties": {}},
        }),
        json!({"name": "app_enable", "description": "Switch an installed mini app on (it starts and is verified again).", "input_schema": id_schema("the one to switch on")}),
        json!({"name": "app_disable", "description": "Switch an installed mini app off: its process stops; its data and settings stay.", "input_schema": id_schema("the one to switch off")}),
        json!({
            "name": "app_uninstall",
            "description": "Ask the user to uninstall a mini app: a card in this chat asks them to confirm. You cannot uninstall it yourself.",
            "input_schema": id_schema("the one to remove"),
        }),
        json!({
            "name": "app_setup",
            "description": "Let the user change, update or fix an installed mini app's setup: its Google client (Client ID and Client Secret), keys or credentials, e.g. when they typed a wrong one or Google refused the client. A card in this chat shows the fields for the user to type into; you never ask for, read or pass the values yourself.",
            "input_schema": id_schema("the one whose setup to change"),
        }),
        json!({
            "name": "app_signin",
            "description": "Sign the user in, log them in, connect or reconnect their Google account (or another account) for an installed mini app: a card in this chat shows Sign in, which the user presses (the browser opens); when the setup it needs (the Google client) is missing, the card asks for that first. Already connected: it says who is signed in and posts nothing. You cannot sign in for them.",
            "input_schema": {"type": "object", "properties": {
                "id": {"type": "string", "description": "The mini app's id (app_list shows them), e.g. app.buwiz.google-workspace"},
                "account": {"type": "string", "description": "The account's id from app_list (e.g. google); its first account when left out"},
            }, "required": ["id"]},
        }),
    ]
}

fn title_of(m: &Value, id: &str) -> String {
    m.get("title").filter(|t| js::truthy(t)).map(js::string_of).unwrap_or_else(|| id.to_owned())
}

/// The card in the turn's chat; `false` when the chat is gone.
pub(super) fn post(run: &Run, n: &Value) -> bool {
    let host = StoreHost::new();
    host.bind(&run.ctx);
    threads::notice(&host, &run.thread_id(), n)
}

/// One of [`NAMES`].
pub async fn tool(run: &Run, name: &str, input: &Value) -> Result<Out, String> {
    let Some(host) = run.ctx.plugins.clone() else { return Ok(Out::err("this broker hosts no mini apps".into())) };
    let id = input.get("id").filter(|v| !v.is_null()).map(js::string_of).unwrap_or_default();
    match name {
        "app_install" => Ok(install(run, &host, input).await),
        "app_list" => Ok(list(&host)),
        "app_setup" => Ok(setup(run, &host, &id)),
        "app_signin" => Ok(signin(run, &host, &id, input.get("account").and_then(Value::as_str).filter(|a| !a.is_empty()))),
        "app_enable" | "app_disable" => {
            let r = if name == "app_enable" { host.enable(&id).await } else { host.disable(&id).await };
            Ok(match r {
                Ok(v) if name == "app_disable" || v.get("ok") == Some(&Value::Bool(true)) => Out::ok(format!("{id} is {}", if name == "app_enable" { "on: its tools return next step" } else { "off: its tools are withheld" })),
                Ok(v) => Out::err(format!("{id} did not start: {} {}", js::string_of(&v["state"]), js::string_of(&v["message"]))),
                Err(e) => Out::err(format!("{name}: {}", e.message)),
            })
        }
        _ => {
            let Some(row) = host.row(&id) else { return Ok(Out::err(format!("{} is not installed (app_list shows the ids)", js::slice_utf16(&id, 80)))) };
            let title = title_of(&row.manifest(), &id);
            let n = json!({"kind": "app_uninstall", "id": id, "title": title, "version": row.version, "state": "pending"});
            if !post(run, &n) {
                return Ok(Out::err("no chat to ask in".into()));
            }
            Ok(Out::ok(format!("a card in this chat asks the user to confirm uninstalling {title}; you cannot uninstall it yourself")))
        }
    }
}

/// Chat `chat`'s card of `kind` for `id` (and `mode`, when it has one) still
/// pending.
pub fn notice_pending(ctx: &HubCtx, chat: &str, kind: &str, id: &str, mode: Option<&str>) -> bool {
    ctx.store.read(|st| {
        st.thread(chat).is_some_and(|t| {
            t.turns.iter().filter_map(|u| u.notice.as_ref()).any(|n| n["kind"] == kind && n["id"] == json!(id) && n["state"] == "pending" && mode.is_none_or(|m| n["mode"] == m))
        })
    })
}

/// An `app_setup` card for `id`'s first setup (its fields, no values) in
/// chat `chat`, unless one is pending there: → (title, provider, whether
/// it is in the chat), or why there is none.
pub fn setup_card(ctx: &HubCtx, host: &Host, chat: &str, id: &str) -> Result<(String, String, bool), String> {
    let Some(row) = host.row(id) else { return Err(format!("{} is not installed (app_list shows the ids)", js::slice_utf16(id, 80))) };
    let title = title_of(&row.manifest(), id);
    let view = host.setup_view(id);
    let Some(s) = view.as_array().and_then(|v| v.first()) else { return Err(format!("{title} has no setup to change")) };
    let provider = js::string_of(&s["provider"]);
    let fields: Vec<Value> =
        s["fields"].as_array().into_iter().flatten().map(|f| json!({"key": f["key"], "label": f["label"], "placeholder": f["placeholder"], "secret": f["secret"] == true, "hint": f["hint"]})).collect();
    let n = json!({"kind": "app_setup", "id": id, "plugin": id, "title": title, "provider": provider, "fields": fields, "state": "pending"});
    let shown = notice_pending(ctx, chat, "app_setup", id, None) || post_in(ctx, chat, &n);
    Ok((title, provider, shown))
}

/// A card in chat `chat`; `false` when the chat is gone.
pub fn post_in(ctx: &HubCtx, chat: &str, n: &Value) -> bool {
    match ctx.store.append_turn(chat, crate::hub::store::Turn::notice(&threads::notice_text(n), n.clone())) {
        Ok(Some(_)) => {
            ctx.store.write_index();
            true
        }
        Ok(None) => false,
        Err(e) => {
            eprintln!("buwiz-broker: notices: the store's write: {e}");
            true
        }
    }
}

/// `app_setup`: an `app_setup` card for `id`'s first setup (its fields,
/// no values), one pending per chat and plugin, ready or not.
fn setup(run: &Run, host: &Host, id: &str) -> Out {
    let (title, provider, shown) = match setup_card(&run.ctx, host, &run.thread_id(), id) {
        Ok(x) => x,
        Err(e) => return Out::err(e),
    };
    if !shown {
        return Out::err("no chat to show the setup card in".into());
    }
    Out::ok(format!(
        "a card in this chat lets the user change {title}'s {}: they type the values into the card. Never ask for the client ID, secret or any key in the chat, and never repeat one",
        super::v1::setup_name(&provider)
    ))
}

/// `app_signin`: the `account_required` card of `id`'s `account` (its
/// first by default) in this chat, one while pending, carrying the setup
/// when it is missing; a connected account is answered with who it is.
fn signin(run: &Run, host: &Host, id: &str, account: Option<&str>) -> Out {
    let Some(row) = host.row(id) else { return Out::err(format!("{} is not installed (app_list shows the ids)", js::slice_utf16(id, 80))) };
    let title = title_of(&row.manifest(), id);
    let declared = host.declared_accounts(id);
    let a = match account {
        Some(want) => declared.into_iter().find(|a| a.id == want),
        None => declared.into_iter().next(),
    };
    let Some(a) = a else {
        return Out::err(match account {
            Some(want) => format!("{title} declares no account {} (app_list shows them)", js::slice_utf16(want, 40)),
            None => format!("{title} has no account to sign in to"),
        });
    };
    let rows = host.account_rows(id);
    let active = host.active_label(id, &a.id);
    if let Some(r) = rows.iter().find(|r| r.account == a.id && r.state == "connected" && active.as_deref() == Some(r.label.as_str())) {
        let who = if r.subject.is_empty() { String::new() } else { format!(" as {}", r.subject) };
        return Out::ok(format!("{title}'s {} is already connected{who}; nothing to do", a.label));
    }
    let chat = run.thread_id();
    let n = super::v1::account_card(id, &title, &a, &host.setup_view(id));
    if !super::v1::card_pending(&run.ctx, &chat, id, &a.id) && !post(run, &n) {
        return Out::err("no chat to show the sign-in card in".into());
    }
    if n.get("setup").is_some() {
        return Out::ok(format!(
            "a card in this chat asks the user to add their {} and then press Sign in for their {}: they type the values into the card. Never ask for the client ID, secret or any key in the chat. You cannot sign in for them",
            super::v1::setup_name(&a.provider),
            a.label
        ));
    }
    Out::ok(format!("a card in this chat shows Sign in for {title}'s {}: the user presses it and signs in in their browser. You cannot sign in for them", a.label))
}

/// `app_list`'s state of a mini app: its setup per provider and its
/// accounts (`account/label state [subject]`); "" when it has neither.
fn state_of(host: &Host, id: &str) -> String {
    let mut parts = Vec::new();
    let setup: Vec<String> =
        host.setup_view(id).as_array().into_iter().flatten().map(|s| format!("{} {}", super::v1::setup_name(&js::string_of(&s["provider"])), if s["ready"] == true { "ready" } else { "missing" })).collect();
    if !setup.is_empty() {
        parts.push(format!("setup: {}", setup.join(", ")));
    }
    let accounts: Vec<String> = host
        .accounts_view(id)
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| {
            let who = a.get("subject").and_then(Value::as_str).filter(|x| !x.is_empty()).map(|x| format!(" as {x}")).unwrap_or_default();
            let active = if a["active"] == true { " (active)" } else { "" };
            format!("{}/{} {}{who}{active}", js::string_of(&a["account"]), js::string_of(&a["label"]), js::string_of(&a["state"]))
        })
        .collect();
    if !accounts.is_empty() {
        parts.push(format!("accounts: {}", accounts.join(", ")));
    }
    parts.iter().map(|p| format!(" · {p}")).collect()
}

fn list(host: &Host) -> Out {
    let rows = host.rows();
    if rows.is_empty() {
        return Out::ok("no mini apps installed".into());
    }
    let lines: Vec<String> = rows
        .iter()
        .map(|r| {
            let state = host.runtime_view(&r.id).state;
            format!("{} — {} · {} · {state} · {}{}", r.id, title_of(&r.manifest(), &r.id), r.version, if r.enabled { "enabled" } else { "disabled" }, state_of(host, &r.id))
        })
        .collect();
    Out::ok(lines.join("\n"))
}

/// A release fetched, verified and inspected for `url` (a mini app's name,
/// a GitHub URL or `owner/repo`, [`release::parse_url`]): the release, the
/// spec to install it with (`package` its folder) and the inspection.
pub async fn fetch_reviewed(ctx: &HubCtx, host: &Host, url: &str) -> Result<(release::Fetched, Value, Value), String> {
    let src = release::parse_url(url)?;
    let api = ctx.ext.get::<GithubApi>().map(|g| g.0.clone()).unwrap_or_else(|| release::GITHUB_API.to_owned());
    let f = release::fetch(&api, &src, &host.trust(), &ctx.data).await?;
    let mut spec = f.spec();
    if f.package.is_some() {
        spec["package"] = json!(f.dir.display().to_string());
    }
    let i = host.inspect(&spec).await.map_err(|e| format!("the release did not pass inspection: {}: {}", e.code, e.message))?;
    Ok((f, spec, i))
}

async fn install(run: &Run, host: &Host, input: &Value) -> Out {
    let url = input.get("url").filter(|v| !v.is_null()).map(js::string_of).unwrap_or_default();
    if let Err(e) = release::parse_url(&url) {
        return Out::err(e);
    }
    run.set_job("connecting", "Fetching the mini app");
    let (f, spec, i) = match fetch_reviewed(&run.ctx, host, &url).await {
        Ok(x) => x,
        Err(e) => return Out::err(format!("app_install: {e}")),
    };
    let m = &i["manifest"];
    let title = title_of(m, &f.id);
    let publisher = m.pointer("/publisher/name").or_else(|| m.pointer("/publisher/id")).filter(|p| js::truthy(p)).map(js::string_of).unwrap_or_default();
    // installed plugins whose env already holds every needed key (keys only)
    let keys: Vec<String> = f.needs_env.iter().map(|n| js::string_of(&n["key"])).collect();
    let env_from: Vec<Value> = if keys.is_empty() {
        Vec::new()
    } else {
        host.rows()
            .into_iter()
            .filter(|r| r.id != f.id)
            .filter(|r| {
                let env = parse(&r.env, json!({}));
                keys.iter().all(|k| env.get(k).is_some_and(Value::is_string))
            })
            .map(|r| json!({"id": r.id, "title": title_of(&r.manifest(), &r.id)}))
            .collect()
    };
    let mut n = json!({
        "kind": "app_install",
        "id": f.id,
        "title": title,
        "version": f.version,
        "publisher": publisher,
        "source": f.source,
        "command": spec["command"],
        "args": spec["args"],
        "env": spec["env"],
        "manifest_sha256": i["manifest_sha256"],
        "review": i["review"],
        "update": i["update"],
        "needs_env": f.needs_env,
        "env_from": env_from,
        "state": "pending",
    });
    if let Some(p) = i.get("package") {
        // the user's Install sends `package` back with the command
        n["package"] = p["dir"].clone();
        n["skills"] = json!(p["skills"].as_array().into_iter().flatten().map(|s| s["name"].clone()).collect::<Vec<_>>());
        n["accounts"] = p["accounts"].clone();
    }
    if !post(run, &n) {
        return Out::err("no chat to show the install card in".into());
    }
    // the setup its accounts lack: asked for in a card after the install
    let mut lacks: Vec<String> = Vec::new();
    for a in n["accounts"].as_array().into_iter().flatten() {
        let p = js::string_of(&a["provider"]);
        let name = super::v1::setup_name(&p);
        if host.setup_missing(&p) && !lacks.contains(&name) {
            lacks.push(name);
        }
    }
    let after = if lacks.is_empty() {
        String::new()
    } else {
        format!(". Once installed it needs the user's {}: a card in this chat asks for it. Never ask for the client ID, secret or any key in the chat: the user types them into the card only", lacks.join(" and "))
    };
    Out::ok(format!("a card in this chat asks the user to review and Install {title} {}; you cannot install it yourself{after}", f.version))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::helper::keystore::Keystore;
    use crate::helper::plugins::{AccountsConfig, Clock as HostClock, Options, Timeouts};
    use crate::helper::release::tests::{index, trust, KEY, SEED};
    use crate::hub::ctx::HubCtx;
    use crate::hub::model::Settings;
    use crate::hub::routes::host::{install_plugin, settle, with_env_from};
    use crate::hub::tools::{exec_tool, tool_list};
    use crate::hub::traits::Clock;
    use crate::hub::types::TurnCtx;

    const FX: &str = "app.buwiz.fixture";

    /// path → (status, location, body)
    type Files = Arc<Mutex<HashMap<String, (u16, String, Vec<u8>)>>>;
    /// `<method> <path>` and the body of each request
    type Log = Arc<Mutex<Vec<(String, String)>>>;

    /// A fake GitHub on 127.0.0.1:0: the API, and assets behind a redirect
    /// (and a token endpoint, whatever `files` says).
    async fn github(files: Files, log: Log) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { return };
                let (files, log) = (files.clone(), log.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match s.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(0) + 4;
                    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
                    let len = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|n| n.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                    while buf.len() < end + len {
                        match s.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let path = head.split(' ').nth(1).unwrap_or("/").to_owned();
                    let method = head.split(' ').next().unwrap_or("").to_owned();
                    log.lock().unwrap().push((format!("{method} {path}"), String::from_utf8_lossy(&buf[end..]).into_owned()));
                    let (status, location, body) = files.lock().unwrap().get(&path).cloned().unwrap_or((404, String::new(), b"{}".to_vec()));
                    let loc = if location.is_empty() { String::new() } else { format!("location: {location}\r\n") };
                    let out = format!("HTTP/1.1 {status} X\r\n{loc}content-length: {}\r\nconnection: close\r\n\r\n", body.len());
                    let _ = s.write_all(out.as_bytes()).await;
                    let _ = s.write_all(&body).await;
                    let _ = s.shutdown().await;
                });
            }
        });
        base
    }

    fn server_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../server/src/v1/fixture")
    }

    fn node() -> String {
        std::env::var("PATH").ok().and_then(|p| p.split(':').map(|d| Path::new(d).join("node")).find(|p| p.exists())).map(|p| p.display().to_string()).unwrap_or_else(|| "node".into())
    }

    fn sha(b: &[u8]) -> String {
        buwiz_protocol::keys::sha256_hex(b)
    }

    /// The fixture plugin as a release: its signed manifest, a launcher
    /// script as the binary, the index; served under tag `tag`.
    fn publish(files: &Files, base: &str, tag: &str, bin_sha: Option<&str>) {
        let dir = server_dir();
        let mut m: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
        m["publisher"]["key_id"] = json!(KEY);
        let sources: Vec<(String, Vec<u8>)> = ["board", "card"].iter().map(|id| (id.to_string(), std::fs::read(dir.join(format!("{id}.tsx"))).unwrap())).collect();
        let manifest = crate::v1::signing::sign_manifest(&m, &buwiz_protocol::keys::b64(&SEED), &sources).unwrap().to_string().into_bytes();
        let bin = format!("#!/bin/sh\nexec '{}' '{}' \"$@\"\n", node(), dir.join("server.mjs").display()).into_bytes();
        let ix = index(FX, &release::platform(), &sha(&manifest), bin_sha.unwrap_or(&sha(&bin)), |ix| {
            ix["run"] = json!({"args": [], "env": {"FX_MANIFEST": "{manifest}"}});
        });
        let mut f = files.lock().unwrap();
        let mut assets = Vec::new();
        for (name, body) in [(release::INDEX_ASSET, ix.into_bytes()), (release::MANIFEST_FILE, manifest), ("fx-bin", bin)] {
            f.insert(format!("/dl/{tag}/{name}"), (302, format!("{base}/obj/{tag}/{name}"), Vec::new()));
            f.insert(format!("/obj/{tag}/{name}"), (200, String::new(), body));
            assets.push(json!({"name": name, "browser_download_url": format!("{base}/dl/{tag}/{name}")}));
        }
        let rel = json!({"tag_name": tag, "assets": assets}).to_string().into_bytes();
        f.insert(format!("/repos/o/r/releases/tags/{tag}"), (200, String::new(), rel.clone()));
        if tag == "v1.0.0" {
            f.insert("/repos/o/r/releases/latest".into(), (200, String::new(), rel));
        }
    }

    async fn bed(label: &str) -> (Arc<HubCtx>, Arc<Host>, Files, PathBuf, String) {
        let b = bed_in(label, HostClock::system()).await;
        (b.ctx, b.host, b.files, b.dir, b.base)
    }

    struct Bed {
        ctx: Arc<HubCtx>,
        host: Arc<Host>,
        files: Files,
        log: Log,
        dir: PathBuf,
        base: String,
        db: PathBuf,
    }

    async fn bed_in(label: &str, clock: HostClock) -> Bed {
        let dir = std::env::temp_dir().join(format!("rox-miniapps-{label}-{}-{}", std::process::id(), crate::hub::store::now_ms()));
        let mut ctx = HubCtx::new(dir.clone(), Clock::manual());
        ctx.store.load().unwrap();
        let trust_file = dir.join("trust.json");
        let t = trust(&[FX, "app.buwiz.other"]);
        let pubs: serde_json::Map<String, Value> = t.publishers.iter().map(|(k, p)| (k.clone(), json!({"alg": "ed25519", "publisher": p.publisher, "public_key": p.public_key}))).collect();
        let approved: Vec<Value> = t.approved.iter().map(|(id, k)| json!({"id": id, "publisher_key_id": k})).collect();
        std::fs::write(&trust_file, json!({"v": 1, "publishers": pubs, "approved_plugins": approved}).to_string()).unwrap();
        let db = dir.join("plugins.db");
        let host = Host::new(Options { db: db.clone(), keys: Keystore::from_spec(&format!("file:{}", dir.join("keys").display())).unwrap(), trust_file, log_dir: None, clock, timeouts: Timeouts::default(), apps_dir: Some(dir.join("v1/apps")) }).unwrap();
        // an installed plugin whose settings the new one may reuse
        let c = rusqlite::Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO v1_plugins(id,short,command,args,env,version,protocol,publisher_key_id,plugin_pub,manifest,manifest_sha256,enabled,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,0,1,1)",
            rusqlite::params!["app.buwiz.other", "ot", "/bin/false", "[]", r#"{"FX_TOKEN":"secret-1"}"#, "1.0.0", "2.0.0", KEY, "pub", r#"{"title":"Other"}"#, "0".repeat(64)],
        )
        .unwrap();
        ctx.plugins = Some(host.clone());
        let (files, log): (Files, Log) = (Arc::default(), Arc::default());
        let base = github(files.clone(), log.clone()).await;
        ctx.ext.insert(GithubApi(base.clone()));
        Bed { ctx: Arc::new(ctx), host, files, log, dir, base, db }
    }

    fn run_in(ctx: &Arc<HubCtx>) -> Run {
        let thread = ctx.store.read(|s| s.active.clone());
        let job = ctx.jobs.create(&thread, Value::Null, None, false);
        Run::new(ctx.clone(), TurnCtx { thread, ..TurnCtx::default() }, job, Arc::new(Settings::from(|_| None)))
    }

    async fn call(run: &Run, name: &str, input: Value) -> Out {
        exec_tool(run, &json!({"name": name, "input": input})).await
    }

    fn last_notice(ctx: &HubCtx, chat: &str) -> (String, Value) {
        ctx.store.read(|s| {
            let u = s.thread(chat).unwrap().turns.iter().rev().find(|u| u.notice.is_some()).unwrap().clone();
            (u.text.unwrap_or_default(), u.notice.unwrap())
        })
    }

    #[test]
    fn the_tools_are_rusts_own_and_not_in_defs_json() {
        let defs_json = include_str!("defs.json");
        for n in NAMES {
            assert!(!defs_json.contains(&format!("\"{n}\"")), "{n} is not in defs.json");
            assert!(defs().iter().any(|d| d["name"] == n), "{n} has a schema");
            assert!(!crate::hub::tools::defs::builtin_names().any(|b| b == n));
        }
    }

    #[tokio::test]
    async fn app_install_fetches_verifies_inspects_and_posts_a_card_the_user_installs() {
        let (ctx, host, files, dir, base) = bed("e2e").await;
        publish(&files, &base, "v1.0.0", None);
        let run = run_in(&ctx);
        let chat = run.thread_id();
        let names: Vec<String> = tool_list(&run).await.iter().map(|t| js::string_of(&t["name"])).collect();
        for n in NAMES {
            assert!(names.contains(&n.to_owned()), "{n} is on the surface");
        }

        assert_eq!(call(&run, "app_install", json!({"url": "https://gitlab.com/o/r"})).await.is_error, Some(true));
        let out = call(&run, "app_install", json!({"url": "o/r"})).await;
        assert_eq!(out, Out::ok("a card in this chat asks the user to review and Install Fixture 1.0.0; you cannot install it yourself".into()));
        let at = dir.join("v1/apps").join(FX).join("1.0.0");
        let (text, n) = last_notice(&ctx, &chat);
        assert_eq!(text, "Install Fixture 1.0.0 from https://github.com/o/r/releases/tag/v1.0.0?");
        assert_eq!((n["kind"].clone(), n["id"].clone(), n["version"].clone(), n["publisher"].clone(), n["state"].clone()), (json!("app_install"), json!(FX), json!("1.0.0"), json!("Goldcoders"), json!("pending")));
        assert_eq!(n["command"], json!(at.join("fx-bin").display().to_string()));
        assert_eq!(n["env"], json!({"FX_MANIFEST": at.join("manifest.signed.json").display().to_string()}));
        assert_eq!(n["needs_env"], json!([{"key": "FX_TOKEN", "label": "Token", "secret": true}]));
        assert_eq!(n["env_from"], json!([{"id": "app.buwiz.other", "title": "Other"}]));
        assert!(n["review"].is_object() && n["update"].is_null() && n["manifest_sha256"].as_str().unwrap().len() == 64, "{n}");
        let keys: Vec<&String> = n.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["kind", "id", "title", "version", "publisher", "source", "command", "args", "env", "manifest_sha256", "review", "update", "needs_env", "env_from", "state"]);
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(at.join("fx-bin")).unwrap().permissions().mode() & 0o777, 0o755);
        assert!(!host.rows().iter().any(|r| r.id == FX), "nothing installed yet");

        // a complete dir is reused: no download of the binary again
        files.lock().unwrap().remove("/obj/v1.0.0/fx-bin");
        assert_eq!(call(&run, "app_install", json!({"url": "https://github.com/o/r/releases/tag/v1.0.0"})).await.is_error, None);
        // a binary that is not the index's: refused, the good dir kept
        publish(&files, &base, "bad", Some(&"c".repeat(64)));
        let bad = call(&run, "app_install", json!({"url": "https://github.com/o/r/releases/tag/bad"})).await;
        assert!(bad.content.as_str().unwrap().contains("fx-bin does not match the index's sha256"), "{bad:?}");
        assert!(at.join("fx-bin").exists());
        let none = call(&run, "app_install", json!({"url": "o/nothing"})).await;
        assert_eq!(none, Out::err("app_install: o/nothing has no release yet".into()));

        // the user's Install: env_from fills the key the card did not
        let body = json!({"command": n["command"], "args": n["args"], "env": n["env"], "manifest_sha256": n["manifest_sha256"], "env_from": "app.buwiz.other", "env_from_keys": ["FX_TOKEN"]});
        let r = install_plugin(&ctx, &host, &body).await.unwrap();
        assert_eq!(r["ok"], true, "{r}");
        let row = host.row(FX).unwrap();
        assert_eq!(parse(&row.env, json!({}))["FX_TOKEN"], "secret-1");
        assert_eq!(last_notice(&ctx, &chat).1["state"], "installed");

        let listed = call(&run, "app_list", json!({})).await;
        assert!(listed.content.as_str().unwrap().contains(&format!("{FX} — Fixture · 1.0.0 · running · enabled")), "{listed:?}");
        assert_eq!(call(&run, "app_disable", json!({"id": FX})).await, Out::ok(format!("{FX} is off: its tools are withheld")));
        assert_eq!(call(&run, "app_enable", json!({"id": FX})).await, Out::ok(format!("{FX} is on: its tools return next step")));
        assert_eq!(call(&run, "app_enable", json!({"id": "app.buwiz.none"})).await.is_error, Some(true));

        // uninstall asks; the card settles through the route
        let u = call(&run, "app_uninstall", json!({"id": FX})).await;
        assert_eq!(u, Out::ok("a card in this chat asks the user to confirm uninstalling Fixture; you cannot uninstall it yourself".into()));
        let (text, n) = last_notice(&ctx, &chat);
        assert_eq!(text, "Uninstall Fixture? It removes its pins, attachments and data.");
        assert_eq!(n, json!({"kind": "app_uninstall", "id": FX, "title": "Fixture", "version": "1.0.0", "state": "pending"}));
        assert!(host.row(FX).is_some(), "still installed");
        assert_eq!(settle(&ctx, &json!({"chat": chat, "kind": "app_uninstall", "id": FX, "state": "declined"})).unwrap_err().code, "invalid_args");
        assert_eq!(settle(&ctx, &json!({"chat": "t404", "kind": "app_uninstall", "id": FX, "state": "cancelled"})).unwrap_err().code, "not_found");
        assert_eq!(settle(&ctx, &json!({"chat": chat, "kind": "routine_consent", "id": FX, "state": "cancelled"})).unwrap_err().code, "invalid_args");
        assert_eq!(settle(&ctx, &json!({"chat": chat, "kind": "app_uninstall", "id": FX, "state": "cancelled"})).unwrap(), json!({"ok": true, "settled": 1}));
        assert_eq!(last_notice(&ctx, &chat).1["state"], "cancelled");
        assert_eq!(settle(&ctx, &json!({"chat": chat, "kind": "app_uninstall", "id": FX, "state": "uninstalled"})).unwrap()["settled"], 0, "only pending ones");
        host.stop_all().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A bare name is `buwiz-app/<slug>`, a URL or `owner/repo` as given,
    /// for the tool and Connect Apps' fetch route alike; a repository
    /// without a release says so.
    #[tokio::test]
    async fn a_bare_name_installs_from_the_first_party_org() {
        let (ctx, host, files, dir, base) = bed("bare").await;
        publish(&files, &base, "v1.0.0", None);
        {
            let mut f = files.lock().unwrap();
            let rel = f["/repos/o/r/releases/latest"].clone();
            f.insert(format!("/repos/{}/google-workspace/releases/latest", release::FIRST_PARTY_ORG), rel.clone());
            f.insert(format!("/repos/{}/word/releases/tags/v1.0.0", release::FIRST_PARTY_ORG), rel);
        }
        let run = run_in(&ctx);
        assert!(defs()[0]["description"].as_str().unwrap().contains("https://github.com/buwiz-app/<name>"));
        let ok = Out::ok("a card in this chat asks the user to review and Install Fixture 1.0.0; you cannot install it yourself".into());
        assert_eq!(call(&run, "app_install", json!({"url": "Google Workspace"})).await, ok);
        assert_eq!(call(&run, "app_install", json!({"url": "word@v1.0.0"})).await, ok);
        assert_eq!(call(&run, "app_install", json!({"url": "o/r"})).await, ok, "owner/repo as given");
        assert_eq!(call(&run, "app_install", json!({"url": "word"})).await, Out::err("app_install: buwiz-app/word has no release yet".into()));
        assert_eq!(call(&run, "app_install", json!({"url": "word 2.0"})).await, Out::err("app_install: buwiz-app/word has no release 2.0".into()));
        // Connect Apps' Add…: the same forms through the host route
        let r = crate::hub::routes::host::fetch_release(&ctx, &host, &json!({"source": "google workspace"})).await.unwrap();
        assert_eq!((r["manifest"]["id"].clone(), r["version"].clone()), (json!(FX), json!("1.0.0")));
        assert_eq!(r["source"], "https://github.com/buwiz-app/google-workspace/releases/tag/v1.0.0");
        assert!(r["command"].as_str().unwrap().ends_with("fx-bin") && r["manifest_sha256"].as_str().unwrap().len() == 64, "{r}");
        let e = crate::hub::routes::host::fetch_release(&ctx, &host, &json!({"source": "word"})).await.unwrap_err();
        assert_eq!((e.code.as_str(), e.message.as_str()), ("not_found", "buwiz-app/word has no release yet"));
        assert_eq!(crate::hub::routes::host::fetch_release(&ctx, &host, &json!({"source": "not a/name/x"})).await.unwrap_err().code, "invalid_args");
        assert!(!host.rows().iter().any(|r| r.id == FX), "nothing installed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn env_from_copies_only_missing_keys_from_an_installed_plugin() {
        let (_ctx, host, _files, dir, _) = bed("envfrom").await;
        let b = |extra: Value| {
            let mut v = json!({"command": "x", "env": {"A": "mine"}});
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            v
        };
        assert_eq!(with_env_from(&host, &b(json!({}))).unwrap(), b(json!({})), "no env_from: unchanged");
        let got = with_env_from(&host, &b(json!({"env_from": "app.buwiz.other", "env_from_keys": ["FX_TOKEN"]}))).unwrap();
        assert_eq!(got, json!({"command": "x", "env": {"A": "mine", "FX_TOKEN": "secret-1"}}));
        let mut typed = b(json!({"env_from": "app.buwiz.other", "env_from_keys": ["FX_TOKEN"]}));
        typed["env"]["FX_TOKEN"] = json!("typed");
        assert_eq!(with_env_from(&host, &typed).unwrap()["env"]["FX_TOKEN"], "typed", "the user's value wins");
        assert_eq!(with_env_from(&host, &b(json!({"env_from": "app.buwiz.none", "env_from_keys": ["FX_TOKEN"]}))).unwrap_err().code, "not_found");
        let missing = with_env_from(&host, &b(json!({"env_from": "app.buwiz.other", "env_from_keys": ["NOPE"]}))).unwrap_err();
        assert_eq!((missing.code.as_str(), missing.message.as_str()), ("invalid_args", "app.buwiz.other has no NOPE"));
        assert_eq!(with_env_from(&host, &b(json!({"env_from": "app.buwiz.other"}))).unwrap_err().code, "invalid_args");
        assert_eq!(with_env_from(&host, &b(json!({"env_from": 5, "env_from_keys": []}))).unwrap_err().code, "invalid_args");
        let _ = std::fs::remove_dir_all(&dir);
    }

    const SKILL: &str = "---\nname: triage\ndescription: Triage the inbox\n---\nRead the newest mail first.\n";

    /// The fixture as a package: plugin.json (signed, with a google
    /// account), the served manifest and a skill as files, the launcher
    /// as this platform's binary; `edit` changes plugin.json before it is
    /// signed.
    fn publish_pkg(files: &Files, base: &str, tag: &str, edit: impl FnOnce(&mut Value)) {
        let dir = server_dir();
        let mut m: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
        m["publisher"]["key_id"] = json!(KEY);
        let sources: Vec<(String, Vec<u8>)> = ["board", "card"].iter().map(|id| (id.to_string(), std::fs::read(dir.join(format!("{id}.tsx"))).unwrap())).collect();
        let manifest = crate::v1::signing::sign_manifest(&m, &buwiz_protocol::keys::b64(&SEED), &sources).unwrap().to_string().into_bytes();
        let bin = format!("#!/bin/sh\nexec '{}' '{}' \"$@\"\n", node(), dir.join("server.mjs").display()).into_bytes();
        let (msha, bsha, ssha) = (sha(&manifest), sha(&bin), sha(SKILL.as_bytes()));
        let plugin = crate::v1::package::tests::plugin_json(&release::platform(), |p| {
            p["mcpServers"]["fx"]["args"] = json!([]);
            p["buwiz"]["files"] = json!({
                "manifest.signed.json": {"asset": "manifest.signed.json", "sha256": msha},
                "skills/triage/SKILL.md": {"asset": "skills.triage.SKILL.md", "sha256": ssha},
            });
            p["buwiz"]["binaries"][release::platform()] = json!({"path": "bin/fx", "asset": "fx-bin", "sha256": bsha});
            edit(p);
        });
        let mut f = files.lock().unwrap();
        let mut assets = Vec::new();
        for (name, body) in [("plugin.json", plugin.to_string().into_bytes()), ("manifest.signed.json", manifest), ("skills.triage.SKILL.md", SKILL.as_bytes().to_vec()), ("fx-bin", bin)] {
            f.insert(format!("/dl/{tag}/{name}"), (302, format!("{base}/obj/{tag}/{name}"), Vec::new()));
            f.insert(format!("/obj/{tag}/{name}"), (200, String::new(), body));
            assets.push(json!({"name": name, "browser_download_url": format!("{base}/dl/{tag}/{name}")}));
        }
        f.insert(format!("/repos/o/r/releases/tags/{tag}"), (200, String::new(), json!({"tag_name": tag, "assets": assets}).to_string().into_bytes()));
    }

    fn token_answer(files: &Files, status: u16, body: Value) {
        files.lock().unwrap().insert("/token".into(), (status, String::new(), body.to_string().into_bytes()));
    }

    fn jwt(claims: Value) -> String {
        use base64::Engine as _;
        format!("e30.{}.sig", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string()))
    }

    async fn until(what: &str, f: impl AsyncFn() -> bool) {
        for _ in 0..200 {
            if f().await {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("timed out waiting for {what}");
    }

    async fn pushed(host: &Host) -> Value {
        let r = host.call_tool(FX, "fx_stats", &json!({}), None, None).await.unwrap();
        r["structuredContent"]["accounts"].clone()
    }

    /// Installs the tag's package through app_install and the user's card.
    async fn install_pkg(b: &Bed, tag: &str) -> Value {
        let run = run_in(&b.ctx);
        let out = call(&run, "app_install", json!({"url": format!("https://github.com/o/r/releases/tag/{tag}")})).await;
        assert_eq!(out.is_error, None, "{out:?}");
        let (_, n) = last_notice(&b.ctx, &run.thread_id());
        let body = json!({"command": n["command"], "args": n["args"], "env": n["env"], "manifest_sha256": n["manifest_sha256"], "package": n["package"]});
        let r = install_plugin(&b.ctx, &b.host, &body).await.unwrap();
        assert_eq!(r["ok"], true, "{r}");
        n
    }

    fn google_client(b: &Bed) {
        let c = rusqlite::Connection::open(&b.db).unwrap();
        c.execute_batch("CREATE TABLE IF NOT EXISTS plugin_settings(plugin TEXT PRIMARY KEY, vals TEXT, updated_at INTEGER)").unwrap();
        c.execute("INSERT OR REPLACE INTO plugin_settings VALUES('@google', ?, 1)", [r#"{"GOOGLE_CLIENT_ID":"cid","GOOGLE_CLIENT_SECRET":"csec"}"#]).unwrap();
    }

    fn accounts_config(b: &Bed, opened: &Arc<Mutex<Vec<String>>>) {
        let o = opened.clone();
        b.host.set_accounts_config(AccountsConfig {
            port: 8787,
            google_authorize: "https://accounts.example.test/auth".into(),
            google_token: format!("{}/token", b.base),
            open: Arc::new(move |u: &str| o.lock().unwrap().push(u.to_owned())),
        });
    }

    /// signin with open: false; → (flow, state) from its authorize URL.
    async fn sign_in(b: &Bed) -> (String, String, url::Url) {
        let r = b.host.account_signin(FX, "google", &json!({"open": false})).await.unwrap();
        let u = url::Url::parse(r["url"].as_str().unwrap()).unwrap();
        let q: HashMap<String, String> = u.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        let flow = q["redirect_uri"].strip_prefix("http://127.0.0.1:8787/oauth/callback/").unwrap();
        let flow = js::decode_uri_component(flow).unwrap();
        (flow, q["state"].clone(), u)
    }

    #[tokio::test]
    async fn a_2_1_package_installs_and_its_google_account_is_pushed_refreshed_and_cleared() {
        let b = bed_in("pkg", HostClock::manual()).await;
        publish_pkg(&b.files, &b.base, "v1.0.0", |_| {});
        let opened = Arc::new(Mutex::new(Vec::new()));
        accounts_config(&b, &opened);
        let n = install_pkg(&b, "v1.0.0").await;
        let at = b.dir.join("v1/apps").join(FX).join("1.0.0");
        assert_eq!(n["package"], json!(at.display().to_string()));
        assert_eq!(n["skills"], json!(["triage"]));
        assert_eq!(n["accounts"], json!([{"id": "google", "provider": "google", "label": "Google account", "required": true}]));
        assert_eq!(n["command"], json!(at.join("bin/fx").display().to_string()));
        assert_eq!(n["env"], json!({"FX_MANIFEST": at.join("manifest.signed.json").display().to_string()}));
        let keys: Vec<&String> = n.as_object().unwrap().keys().collect();
        assert_eq!(&keys[keys.len() - 4..], ["state", "package", "skills", "accounts"]);
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(at.join("bin/fx")).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read_to_string(at.join("skills/triage/SKILL.md")).unwrap(), SKILL);
        assert!(at.join("plugin.json").exists());
        let row = b.host.row(FX).unwrap();
        assert_eq!(row.package.as_deref(), Some(at.display().to_string().as_str()));
        assert_eq!(parse(&row.skills, json!([])), json!([{"name": "triage", "description": "Triage the inbox", "file": at.join("skills/triage/SKILL.md").display().to_string()}]));
        assert_eq!(b.host.runtime_view(FX).state, "running");

        // no Google client yet: the row says so, sign-in refuses
        let view = b.host.accounts_view(FX);
        assert_eq!(view, json!([{"account": "google", "label": "default", "provider": "google", "title": "Google account", "subject": null, "state": "none", "required": true, "setup": "Add your Google client in Setup"}]));
        let e = b.host.account_signin(FX, "google", &json!({})).await.unwrap_err();
        assert_eq!((e.code.as_str(), e.message.as_str()), ("needs_setup", "Add your Google client in Setup"));
        assert_eq!(b.host.account_signin(FX, "github", &json!({})).await.unwrap_err().code, "not_found");
        google_client(&b);

        let (flow, state, u) = sign_in(&b).await;
        assert_eq!(flow, "account:app.buwiz.fixture/google/default");
        let q: HashMap<String, String> = u.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        assert_eq!(u.host_str(), Some("accounts.example.test"));
        assert_eq!((q["client_id"].as_str(), q["access_type"].as_str(), q["prompt"].as_str(), q["code_challenge_method"].as_str(), q["response_type"].as_str()), ("cid", "offline", "consent", "S256", "code"));
        assert_eq!(q["scope"], "https://www.googleapis.com/auth/gmail.readonly openid email");
        assert!(opened.lock().unwrap().is_empty(), "open: false opens nothing");
        assert_eq!(b.host.accounts_view(FX), json!([{"account": "google", "label": "default", "provider": "google", "title": "Google account", "subject": null, "state": "none", "required": true}]));
        assert!(b.host.account_pending(&flow));
        assert_eq!(b.host.account_callback(&flow, Some("code-1"), Some("wrong"), None).await.0, "refused");
        assert_eq!(b.host.account_callback("account:app.buwiz.fixture/google/other", Some("code-1"), Some(&state), None).await.0, "refused");

        token_answer(&b.files, 200, json!({"access_token": "at-1", "refresh_token": "rt-1", "expires_in": 3600, "token_type": "Bearer", "scope": "https://www.googleapis.com/auth/gmail.readonly openid email", "id_token": jwt(json!({"email": "u@example.com"}))}));
        assert_eq!(b.host.account_callback(&flow, Some("code-1"), Some(&state), None).await, ("connected", String::new()));
        assert!(!b.host.account_pending(&flow));
        let (_, body) = b.log.lock().unwrap().iter().rev().find(|(r, _)| r == "POST /token").cloned().unwrap();
        let form: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes()).into_owned().collect();
        assert_eq!((form["grant_type"].as_str(), form["code"].as_str(), form["client_id"].as_str(), form["client_secret"].as_str()), ("authorization_code", "code-1", "cid", "csec"));
        assert_eq!(form["redirect_uri"], "http://127.0.0.1:8787/oauth/callback/account%3Aapp.buwiz.fixture%2Fgoogle%2Fdefault");
        assert_eq!(buwiz_protocol::keys::sha256_hex(form["code_verifier"].as_bytes()).len(), 64);
        let tok = |t: &str| buwiz_protocol::keys::sha256_hex(t.as_bytes())[..12].to_owned();
        assert_eq!(pushed(&b.host).await, json!({"google/default": {"subject": "u@example.com", "expires_at": 3600, "token": tok("at-1"), "sets": 1}}));
        let rows = b.host.account_rows(FX);
        assert_eq!((rows[0].state.as_str(), rows[0].subject.as_str(), rows[0].token_ref.as_str()), ("connected", "u@example.com", "app.buwiz.fixture/google/default"));
        assert_eq!(rows[0].scopes, ["https://www.googleapis.com/auth/gmail.readonly", "openid", "email"]);
        let keys = Keystore::from_spec(&format!("file:{}", b.dir.join("keys").display())).unwrap();
        assert_eq!(keys.get_token("app.buwiz.fixture/google/default").unwrap().unwrap()["refresh_token"], "rt-1");
        // the views carry no token
        let listed = crate::hub::routines::plugins::describe(&b.host, &b.host.row(FX).unwrap(), &|_, _| json!({}));
        assert_eq!(listed["accounts"], json!([{"account": "google", "label": "default", "provider": "google", "title": "Google account", "subject": "u@example.com", "state": "connected", "required": true, "active": true}]));
        assert_eq!(listed["package"], json!(at.display().to_string()));
        assert_eq!(listed["skills"], json!([{"name": "triage", "description": "Triage the inbox"}]));
        let text = listed.to_string();
        assert!(!text.contains("at-1") && !text.contains("rt-1"), "{text}");

        // 5 minutes before expiry: refreshed and pushed again
        token_answer(&b.files, 200, json!({"access_token": "at-2", "expires_in": 3600, "token_type": "Bearer"}));
        assert!(b.host.clock.manual_step(Some(3_300_000.0), true));
        until("the refreshed token", async || pushed(&b.host).await["google/default"]["token"] == json!(tok("at-2"))).await;
        let (_, body) = b.log.lock().unwrap().iter().rev().find(|(r, _)| r == "POST /token").cloned().unwrap();
        let form: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes()).into_owned().collect();
        assert_eq!((form["grant_type"].as_str(), form["refresh_token"].as_str()), ("refresh_token", "rt-1"));
        assert_eq!(pushed(&b.host).await["google/default"]["expires_at"], 6900);
        assert_eq!(keys.get_token("app.buwiz.fixture/google/default").unwrap().unwrap()["refresh_token"], "rt-1", "the old refresh token stays");

        // a new process gets the account after provision
        b.host.disable(FX).await.unwrap();
        assert_eq!(b.host.enable(FX).await.unwrap()["state"], "running");
        assert_eq!(pushed(&b.host).await, json!({"google/default": {"subject": "u@example.com", "expires_at": 6900, "token": tok("at-2"), "sets": 1}}));

        // a failed refresh: expired, and cleared in the plugin
        token_answer(&b.files, 400, json!({"error": "invalid_grant"}));
        assert!(b.host.clock.manual_step(Some(6_600_000.0), true));
        until("the clear", async || pushed(&b.host).await == json!({})).await;
        assert_eq!(b.host.account_rows(FX)[0].state, "expired");
        assert_eq!(b.host.accounts_view(FX)[0]["state"], "expired");

        // sign out, then remove
        assert_eq!(b.host.account_signout(FX, "google", &json!({})).await.unwrap()["state"], "none");
        assert_eq!(b.host.account_rows(FX)[0].state, "none");
        assert_eq!(keys.get_token("app.buwiz.fixture/google/default").unwrap(), None);
        assert_eq!(b.host.account_remove(FX, "google", &json!({"label": "default"})).await.unwrap()["state"], "removed");
        assert!(b.host.account_rows(FX).is_empty());
        assert_eq!(b.host.accounts_view(FX)[0]["state"], "none", "declared, never signed in: a default row");
        assert_eq!(b.host.account_remove(FX, "google", &json!({})).await.unwrap_err().code, "not_found");

        // uninstall takes the folder and every account with it
        let (flow, state, _) = sign_in(&b).await;
        token_answer(&b.files, 200, json!({"access_token": "at-3", "refresh_token": "rt-3", "expires_in": 3600, "token_type": "Bearer"}));
        assert_eq!(b.host.account_callback(&flow, Some("c"), Some(&state), None).await.0, "connected");
        b.host.uninstall(FX).await.unwrap();
        assert!(!b.dir.join("v1/apps").join(FX).exists(), "the app's folder went");
        assert!(b.host.account_rows(FX).is_empty());
        assert_eq!(keys.get_token("app.buwiz.fixture/google/default").unwrap(), None);
        b.host.stop_all().await;
        let _ = std::fs::remove_dir_all(&b.dir);
    }

    fn cards(ctx: &HubCtx, chat: &str, kind: &str) -> Vec<(String, Value)> {
        ctx.store.read(|s| {
            s.thread(chat).unwrap().turns.iter().filter_map(|u| u.notice.clone().filter(|n| n["kind"] == kind).map(|n| (u.text.clone().unwrap_or_default(), n))).collect()
        })
    }

    fn stored_google(b: &Bed) -> Value {
        use rusqlite::OptionalExtension as _;
        let c = rusqlite::Connection::open(&b.db).unwrap();
        c.execute_batch("CREATE TABLE IF NOT EXISTS plugin_settings(plugin TEXT PRIMARY KEY, vals TEXT, updated_at INTEGER)").unwrap();
        let v: Option<String> = c.query_row("SELECT vals FROM plugin_settings WHERE plugin='@google'", [], |r| r.get(0)).optional().unwrap();
        v.map(|v| serde_json::from_str::<Value>(&v).unwrap()).unwrap_or(Value::Null)
    }

    /// No shared Google client: the install asks for it in the chat (one
    /// card carrying the fields, no values), the views list the fields
    /// with only whether each is set, the save takes only those fields and
    /// keeps a stored secret on an empty one, and the sign-in settles the
    /// card.
    #[tokio::test]
    async fn a_package_without_its_google_client_asks_for_setup_in_the_chat_and_saves_it() {
        let b = bed_in("setup", HostClock::manual()).await;
        publish_pkg(&b.files, &b.base, "v1.0.0", |_| {});
        accounts_config(&b, &Arc::new(Mutex::new(Vec::new())));
        let run = run_in(&b.ctx);
        let chat = run.thread_id();
        let out = call(&run, "app_install", json!({"url": "https://github.com/o/r/releases/tag/v1.0.0"})).await;
        assert!(out.content.as_str().unwrap().ends_with("you cannot install it yourself. Once installed it needs the user's Google client: a card in this chat asks for it. Never ask for the client ID, secret or any key in the chat: the user types them into the card only"), "{out:?}");
        install_pkg(&b, "v1.0.0").await;
        let asked = cards(&b.ctx, &chat, "account_required");
        assert_eq!(asked.len(), 1, "one setup card: {asked:?}");
        let (text, n) = asked[0].clone();
        assert_eq!(text, "Set up Google for Fixture");
        let fields = n["setup"]["fields"].as_array().unwrap();
        assert_eq!(n["setup"]["provider"], "google");
        assert_eq!(fields.iter().map(|f| (js::string_of(&f["key"]), f["secret"] == true)).collect::<Vec<_>>(), [("GOOGLE_CLIENT_ID".into(), false), ("GOOGLE_CLIENT_SECRET".into(), true)]);
        assert!(fields.iter().all(|f| f.as_object().unwrap().keys().eq(["key", "label", "placeholder", "secret", "hint"].iter())), "{fields:?}");
        assert_eq!((n["id"].clone(), n["account"].clone(), n["state"].clone()), (json!(FX), json!("google"), json!("pending")));
        // another install of it: still the one card
        assert_eq!(crate::hub::routes::host::ask_setup(&b.ctx, &b.host, &chat, FX), 0);

        let set = |v: &Value| v[0]["fields"].as_array().unwrap().iter().map(|f| f["set"] == true).collect::<Vec<_>>();
        let view = b.host.setup_view(FX);
        assert_eq!((view[0]["provider"].clone(), view[0]["ready"].clone(), set(&view)), (json!("google"), json!(false), vec![false, false]));
        assert_eq!(view[0]["fields"][1]["label"], "Google Client Secret");
        let listed = crate::hub::routines::plugins::describe(&b.host, &b.host.row(FX).unwrap(), &|_, _| json!({}));
        assert_eq!(listed["setup"], view);

        // only the declared fields, strings; nothing saved on a refusal
        let e = b.host.save_setup(FX, Some(&json!({"GOOGLE_CLIENT_ID": "cid", "OTHER_KEY": "x"})), false).await.unwrap_err();
        assert_eq!((e.code.as_str(), e.message.as_str()), ("invalid_args", "app.buwiz.fixture's setup has no field OTHER_KEY"));
        assert_eq!(b.host.save_setup(FX, Some(&json!({"GOOGLE_CLIENT_ID": 5})), false).await.unwrap_err().code, "invalid_args");
        assert_eq!(b.host.save_setup(FX, Some(&json!(["x"])), false).await.unwrap_err().code, "invalid_args");
        assert_eq!(b.host.save_setup("app.buwiz.other", Some(&json!({})), false).await.unwrap_err().message, "app.buwiz.other needs no setup");
        assert_eq!(b.host.save_setup("app.buwiz.none", Some(&json!({})), false).await.unwrap_err().code, "not_found");
        assert_eq!(stored_google(&b), Value::Null);

        let mut events = b.host.events();
        let r = crate::hub::routes::host::save_setup(&b.ctx, &b.host, FX, &json!({"values": {"GOOGLE_CLIENT_ID": " cid ", "GOOGLE_CLIENT_SECRET": "csec"}})).await.unwrap();
        assert!(!r.to_string().contains("csec"), "no secret answered: {r}");
        assert_eq!(r["setup"][0]["fields"][0]["value"], "cid", "the Client ID shows, to be changed");
        assert!(r["setup"][0]["fields"][1].get("value").is_none());
        assert_eq!((r["setup"][0]["ready"].clone(), set(&r["setup"])), (json!(true), vec![true, true]));
        assert!(r["accounts"][0].get("setup").is_none(), "{r}");
        assert_eq!(events.try_recv().unwrap()["id"], FX, "the plugin's change event");
        assert_eq!(stored_google(&b), json!({"GOOGLE_CLIENT_ID": "cid", "GOOGLE_CLIENT_SECRET": "csec"}));
        // an empty secret keeps the stored one; the id is replaced
        b.host.save_setup(FX, Some(&json!({"GOOGLE_CLIENT_ID": "cid-2", "GOOGLE_CLIENT_SECRET": "  "})), false).await.unwrap();
        assert_eq!(stored_google(&b), json!({"GOOGLE_CLIENT_ID": "cid-2", "GOOGLE_CLIENT_SECRET": "csec"}));
        assert!(!b.host.setup_missing("google"));

        // the sign-in it offers next settles the card
        let (flow, state, u) = sign_in(&b).await;
        assert!(u.query_pairs().any(|(k, v)| k == "client_id" && v == "cid-2"));
        token_answer(&b.files, 200, json!({"access_token": "at-1", "refresh_token": "rt-1", "expires_in": 3600, "token_type": "Bearer"}));
        assert_eq!(b.host.account_callback(&flow, Some("c"), Some(&state), None).await.0, "connected");
        assert_eq!(crate::hub::routes::host::settle_signed_in(&b.ctx, FX, "google"), 1);
        assert_eq!(cards(&b.ctx, &chat, "account_required")[0].1["state"], "connected");
        // Not now on a setup card is `declined`
        b.host.save_setup(FX, Some(&json!({"GOOGLE_CLIENT_ID": ""})), false).await.unwrap();
        assert!(b.host.setup_missing("google"));
        assert_eq!(crate::hub::routes::host::ask_setup(&b.ctx, &b.host, &chat, FX), 1);
        assert_eq!(settle(&b.ctx, &json!({"chat": chat, "kind": "account_required", "id": FX, "state": "declined"})).unwrap()["settled"], 1);
        b.host.stop_all().await;
        let _ = std::fs::remove_dir_all(&b.dir);
    }

    #[tokio::test]
    async fn a_package_whose_google_client_is_set_installs_without_a_setup_card() {
        let b = bed_in("setupdone", HostClock::manual()).await;
        publish_pkg(&b.files, &b.base, "v1.0.0", |_| {});
        google_client(&b);
        let run = run_in(&b.ctx);
        let out = call(&run, "app_install", json!({"url": "https://github.com/o/r/releases/tag/v1.0.0"})).await;
        assert!(out.content.as_str().unwrap().ends_with("you cannot install it yourself"), "{out:?}");
        install_pkg(&b, "v1.0.0").await;
        assert!(cards(&b.ctx, &run.thread_id(), "account_required").is_empty());
        assert_eq!(b.host.setup_view(FX)[0]["ready"], true);
        b.host.stop_all().await;
        let _ = std::fs::remove_dir_all(&b.dir);
    }

    /// Signs FX's google account in with token `at`.
    async fn connect(b: &Bed, at: &str) {
        let (flow, state, _) = sign_in(b).await;
        token_answer(&b.files, 200, json!({"access_token": at, "refresh_token": "rt", "expires_in": 3600, "token_type": "Bearer"}));
        assert_eq!(b.host.account_callback(&flow, Some("c"), Some(&state), None).await.0, "connected");
    }

    /// A saved client can change: the same values change nothing, a new
    /// secret expires the Google sign-ins (cleared in the plugin), and
    /// `clear` removes both values, the setup missing again.
    #[tokio::test]
    async fn a_changed_or_removed_google_client_expires_the_google_sign_ins() {
        let b = bed_in("setupchange", HostClock::manual()).await;
        publish_pkg(&b.files, &b.base, "v1.0.0", |_| {});
        accounts_config(&b, &Arc::new(Mutex::new(Vec::new())));
        install_pkg(&b, "v1.0.0").await;
        google_client(&b);
        connect(&b, "at-1").await;
        assert_eq!(pushed(&b.host).await["google/default"]["sets"], 1);
        // the same values: nothing changed, still signed in
        b.host.save_setup(FX, Some(&json!({"GOOGLE_CLIENT_ID": "cid", "GOOGLE_CLIENT_SECRET": "csec"})), false).await.unwrap();
        b.host.save_setup(FX, Some(&json!({"GOOGLE_CLIENT_SECRET": ""})), false).await.unwrap();
        assert_eq!(b.host.account_rows(FX)[0].state, "connected");
        // a new secret: expired and cleared
        let mut accounts = b.host.account_events();
        b.host.save_setup(FX, Some(&json!({"GOOGLE_CLIENT_SECRET": "csec-2"})), false).await.unwrap();
        assert_eq!(b.host.account_rows(FX)[0].state, "expired");
        assert_eq!(b.host.accounts_view(FX)[0]["state"], "expired");
        assert_eq!(pushed(&b.host).await, json!({}), "buwiz.account.clear reached the plugin");
        assert_eq!(accounts.try_recv().unwrap()["state"], "expired");
        // signed in again, then the client removed
        connect(&b, "at-2").await;
        let r = crate::hub::routes::host::save_setup(&b.ctx, &b.host, FX, &json!({"clear": true})).await.unwrap();
        assert_eq!((r["setup"][0]["ready"].clone(), r["accounts"][0]["state"].clone()), (json!(false), json!("expired")));
        assert_eq!(stored_google(&b), Value::Null, "both values gone");
        assert!(b.host.setup_missing("google"));
        assert_eq!(pushed(&b.host).await, json!({}));
        b.host.stop_all().await;
        let _ = std::fs::remove_dir_all(&b.dir);
    }

    /// Google answering `invalid_client` to the code exchange: the row says
    /// the client was refused and that the setup fixes it; the next
    /// sign-in starts clean.
    #[tokio::test]
    async fn a_refused_google_client_says_so_on_the_account_row() {
        let b = bed_in("badclient", HostClock::manual()).await;
        publish_pkg(&b.files, &b.base, "v1.0.0", |_| {});
        accounts_config(&b, &Arc::new(Mutex::new(Vec::new())));
        install_pkg(&b, "v1.0.0").await;
        google_client(&b);
        let (flow, state, _) = sign_in(&b).await;
        token_answer(&b.files, 401, json!({"error": "invalid_client", "error_description": "The OAuth client was not found."}));
        let got = b.host.account_callback(&flow, Some("c"), Some(&state), None).await;
        assert_eq!(got, ("failed", "Google refused this client. Check the Client ID and Secret.".to_owned()));
        let row = b.host.accounts_view(FX)[0].clone();
        assert_eq!((row["error"].clone(), row["fix"].clone()), (json!("Google refused this client. Check the Client ID and Secret."), json!("setup")));
        // another failure is said as it is, without the setup fix
        let (flow, state, _) = sign_in(&b).await;
        assert!(b.host.accounts_view(FX)[0].get("error").is_none(), "a new sign-in starts clean");
        token_answer(&b.files, 400, json!({"error": "invalid_grant", "error_description": "Bad code"}));
        b.host.account_callback(&flow, Some("c"), Some(&state), None).await;
        let row = b.host.accounts_view(FX)[0].clone();
        assert_eq!((row["error"].clone(), row.get("fix")), (json!("Bad code"), None));
        b.host.stop_all().await;
        let _ = std::fs::remove_dir_all(&b.dir);
    }

    /// `app_setup`: a card with the setup's fields (no values) even when it
    /// is ready, one while pending; a save settles it `saved`, the user's
    /// Cancel `cancelled`.
    #[tokio::test]
    async fn app_setup_puts_a_card_to_change_the_setup_in_the_chat() {
        let b = bed_in("appsetup", HostClock::manual()).await;
        publish_pkg(&b.files, &b.base, "v1.0.0", |_| {});
        google_client(&b);
        install_pkg(&b, "v1.0.0").await;
        let run = run_in(&b.ctx);
        let chat = run.thread_id();
        assert!(defs().iter().any(|d| d["name"] == "app_setup" && d["description"].as_str().unwrap().contains("change, update or fix")));
        let out = call(&run, "app_setup", json!({"id": FX})).await;
        assert_eq!(out, Out::ok("a card in this chat lets the user change Fixture's Google client: they type the values into the card. Never ask for the client ID, secret or any key in the chat, and never repeat one".into()));
        let got = cards(&b.ctx, &chat, "app_setup");
        assert_eq!(got.len(), 1);
        let (text, n) = got[0].clone();
        assert_eq!(text, "Change the Google client of Fixture");
        assert_eq!((n["id"].clone(), n["provider"].clone(), n["state"].clone()), (json!(FX), json!("google"), json!("pending")));
        assert!(!n.to_string().contains("csec") && !n.to_string().contains("\"cid\""), "no values in the chat: {n}");
        assert_eq!(n["fields"].as_array().unwrap().len(), 2);
        call(&run, "app_setup", json!({"id": FX})).await;
        assert_eq!(cards(&b.ctx, &chat, "app_setup").len(), 1, "one while pending");
        assert_eq!(call(&run, "app_setup", json!({"id": "app.buwiz.other"})).await, Out::err("Other has no setup to change".into()));
        assert_eq!(call(&run, "app_setup", json!({"id": "app.buwiz.none"})).await.is_error, Some(true));
        crate::hub::routes::host::save_setup(&b.ctx, &b.host, FX, &json!({"values": {"GOOGLE_CLIENT_SECRET": "csec-2"}})).await.unwrap();
        assert_eq!(cards(&b.ctx, &chat, "app_setup")[0].1["state"], "saved");
        call(&run, "app_setup", json!({"id": FX})).await;
        assert_eq!(settle(&b.ctx, &json!({"chat": chat, "kind": "app_setup", "id": FX, "state": "cancelled"})).unwrap()["settled"], 1);
        assert_eq!(settle(&b.ctx, &json!({"chat": chat, "kind": "app_setup", "id": FX, "state": "declined"})).unwrap_err().code, "invalid_args");
        b.host.stop_all().await;
        let _ = std::fs::remove_dir_all(&b.dir);
    }

    /// `app_signin` and `app_list`: the list tells needs-setup from
    /// needs-sign-in from connected; the tool posts the sign-in card (with
    /// the setup while it is missing), one while pending, and posts none
    /// once connected.
    #[tokio::test]
    async fn app_signin_posts_the_sign_in_card_and_app_list_says_the_setup_and_accounts() {
        let b = bed_in("appsignin", HostClock::manual()).await;
        publish_pkg(&b.files, &b.base, "v1.0.0", |_| {});
        accounts_config(&b, &Arc::new(Mutex::new(Vec::new())));
        install_pkg(&b, "v1.0.0").await;
        let run = run_in(&b.ctx);
        let chat = run.thread_id();
        let list = || async { call(&run, "app_list", json!({})).await.content.as_str().unwrap().to_owned() };
        assert!(list().await.contains(&format!("{FX} — Fixture · 1.0.0 · running · enabled · setup: Google client missing · accounts: google/default none")), "{}", list().await);
        assert!(defs().iter().any(|d| d["name"] == "app_signin" && d["description"].as_str().unwrap().contains("log them in")));
        // the install's setup card is the pending one: no second card
        let before = cards(&b.ctx, &chat, "account_required").len();
        let out = call(&run, "app_signin", json!({"id": FX})).await;
        assert_eq!(out, Out::ok("a card in this chat asks the user to add their Google client and then press Sign in for their Google account: they type the values into the card. Never ask for the client ID, secret or any key in the chat. You cannot sign in for them".into()));
        assert_eq!(cards(&b.ctx, &chat, "account_required").len(), before);
        assert_eq!(before, 1);
        assert!(cards(&b.ctx, &chat, "account_required")[0].1.get("setup").is_some());
        // set up, the card declined: a plain Sign in card
        google_client(&b);
        settle(&b.ctx, &json!({"chat": chat, "kind": "account_required", "id": FX, "state": "declined"})).unwrap();
        assert!(list().await.contains("setup: Google client ready · accounts: google/default none"));
        let out = call(&run, "app_signin", json!({"id": FX, "account": "google"})).await;
        assert_eq!(out, Out::ok("a card in this chat shows Sign in for Fixture's Google account: the user presses it and signs in in their browser. You cannot sign in for them".into()));
        let got = cards(&b.ctx, &chat, "account_required");
        assert_eq!(got.len(), 2);
        assert_eq!((got[1].0.as_str(), got[1].1.get("setup")), ("Sign in to Google for Fixture", None));
        call(&run, "app_signin", json!({"id": FX})).await;
        assert_eq!(cards(&b.ctx, &chat, "account_required").len(), 2, "one while pending");
        // connected: who, and no card
        let (flow, state, _) = sign_in(&b).await;
        token_answer(&b.files, 200, json!({"access_token": "at", "refresh_token": "rt", "expires_in": 3600, "token_type": "Bearer", "id_token": jwt(json!({"email": "u@example.com"}))}));
        assert_eq!(b.host.account_callback(&flow, Some("c"), Some(&state), None).await.0, "connected");
        assert_eq!(crate::hub::routes::host::settle_signed_in(&b.ctx, FX, "google"), 1);
        assert_eq!(call(&run, "app_signin", json!({"id": FX})).await, Out::ok("Fixture's Google account is already connected as u@example.com; nothing to do".into()));
        assert_eq!(cards(&b.ctx, &chat, "account_required").len(), 2);
        assert!(list().await.contains("accounts: google/default connected as u@example.com"));
        assert!(!list().await.contains("csec") && !list().await.contains("\"cid"));
        assert_eq!(call(&run, "app_signin", json!({"id": FX, "account": "github"})).await, Out::err("Fixture declares no account github (app_list shows them)".into()));
        assert_eq!(call(&run, "app_signin", json!({"id": "app.buwiz.other"})).await, Out::err("Other has no account to sign in to".into()));
        assert_eq!(call(&run, "app_signin", json!({"id": "app.buwiz.none"})).await.is_error, Some(true));
        b.host.stop_all().await;
        let _ = std::fs::remove_dir_all(&b.dir);
    }

    /// Signs FX's google account in under `label` (token `at`).
    async fn connect_as(b: &Bed, label: &str, at: &str, who: &str) {
        let r = b.host.account_signin(FX, "google", &json!({"open": false, "label": label})).await.unwrap();
        let u = url::Url::parse(r["url"].as_str().unwrap()).unwrap();
        let q: HashMap<String, String> = u.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        let flow = js::decode_uri_component(q["redirect_uri"].strip_prefix("http://127.0.0.1:8787/oauth/callback/").unwrap()).unwrap();
        token_answer(&b.files, 200, json!({"access_token": at, "refresh_token": "rt", "expires_in": 3600, "token_type": "Bearer", "id_token": jwt(json!({"email": who}))}));
        assert_eq!(b.host.account_callback(&flow, Some("c"), Some(&q["state"]), None).await.0, "connected");
    }

    async fn pushed_labels(b: &Bed) -> Vec<String> {
        pushed(&b.host).await.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
    }

    /// `@fx:login|logout|switch|setup`: offered for a package with
    /// accounts only, each a card in the chat; several signed-in labels
    /// with one active, only it pushed: a new sign-in, a switch and a
    /// sign-out of the active one each move it, the others cleared.
    #[tokio::test]
    async fn account_commands_post_their_cards_and_only_the_active_label_is_pushed() {
        use crate::hub::routes::host::account_command;
        let b = bed_in("acctcmd", HostClock::manual()).await;
        publish_pkg(&b.files, &b.base, "v1.0.0", |_| {});
        accounts_config(&b, &Arc::new(Mutex::new(Vec::new())));
        google_client(&b);
        install_pkg(&b, "v1.0.0").await;
        let run = run_in(&b.ctx);
        let chat = run.thread_id();
        let describe = |id: &str| crate::hub::routines::plugins::describe(&b.host, &b.host.row(id).unwrap(), &|_, _| json!({}));
        assert_eq!(describe(FX)["account_commands"], json!(["login", "logout", "switch", "setup"]));
        assert!(describe("app.buwiz.other").get("account_commands").is_none(), "no accounts, no commands");
        let cmd = |c: &str| account_command(&b.ctx, &b.host, FX, &json!({"chat": chat, "command": c}));
        // nobody signed in: login is the sign-in card, logout says so
        assert_eq!(cmd("login").unwrap(), json!({"ok": true, "posted": true, "kind": "account_required"}));
        cmd("login").unwrap();
        assert_eq!(cards(&b.ctx, &chat, "account_required").len(), 1, "one while pending");
        assert_eq!(cmd("logout").unwrap(), json!({"ok": true, "posted": false, "kind": "app_accounts", "message": "No Google account is signed in for Fixture"}));
        assert_eq!(cmd("setup").unwrap()["kind"], "app_setup");
        assert_eq!(cards(&b.ctx, &chat, "app_setup").len(), 1);
        assert_eq!(cmd("nope").unwrap_err().code, "invalid_args");
        assert_eq!(account_command(&b.ctx, &b.host, "app.buwiz.other", &json!({"chat": chat, "command": "login"})).unwrap_err().code, "invalid_args");
        assert_eq!(account_command(&b.ctx, &b.host, FX, &json!({"chat": "t404", "command": "login"})).unwrap_err().code, "not_found");

        // two sign-ins: the newer is active, alone in the plugin
        connect_as(&b, "default", "at-d", "d@example.com").await;
        assert_eq!(pushed_labels(&b).await, ["google/default"]);
        connect_as(&b, "work", "at-w", "w@example.com").await;
        assert_eq!(pushed_labels(&b).await, ["google/work"], "the other label is cleared");
        let view = b.host.accounts_view(FX);
        assert_eq!(view.as_array().unwrap().iter().map(|r| (js::string_of(&r["label"]), r["active"] == true)).collect::<Vec<_>>(), [("default".into(), false), ("work".into(), true)]);
        assert!(call(&run, "app_list", json!({})).await.content.as_str().unwrap().contains("accounts: google/default connected as d@example.com, google/work connected as w@example.com (active)"));
        assert_eq!(call(&run, "app_signin", json!({"id": FX})).await.content, json!("Fixture's Google account is already connected as w@example.com; nothing to do"));
        // switch
        assert_eq!(b.host.account_use(FX, "google", &json!({"label": "default"})).await.unwrap()["label"], "default");
        assert_eq!(pushed_labels(&b).await, ["google/default"]);
        assert_eq!(b.host.active_label(FX, "google").as_deref(), Some("default"));
        assert_eq!(b.host.account_use(FX, "google", &json!({"label": "nobody"})).await.unwrap_err().code, "not_found");
        // a new process gets only the active one
        b.host.disable(FX).await.unwrap();
        b.host.enable(FX).await.unwrap();
        assert_eq!(pushed_labels(&b).await, ["google/default"]);

        // signed in: login, logout and switch are app_accounts cards of their mode
        for (c, text) in [("login", "Your Google account for Fixture"), ("logout", "Sign out of Google for Fixture?"), ("switch", "Switch the Google account for Fixture")] {
            assert_eq!(cmd(c).unwrap(), json!({"ok": true, "posted": true, "kind": "app_accounts"}));
            let (t, n) = cards(&b.ctx, &chat, "app_accounts").pop().unwrap();
            assert_eq!((t.as_str(), n["mode"].clone(), n["account"].clone(), n["state"].clone()), (text, json!(c), json!("google"), json!("pending")));
            assert!(!n.to_string().contains("example.com"), "no subject in the card: {n}");
        }
        cmd("switch").unwrap();
        assert_eq!(cards(&b.ctx, &chat, "app_accounts").len(), 3, "one of each mode while pending");
        assert_eq!(settle(&b.ctx, &json!({"chat": chat, "kind": "app_accounts", "id": FX, "state": "done"})).unwrap()["settled"], 3);

        // the active one signed out: the other is active and pushed
        let out = b.host.account_signout(FX, "google", &json!({"label": "default"})).await.unwrap();
        assert_eq!(out["active"], "work");
        assert_eq!(pushed_labels(&b).await, ["google/work"]);
        // a non-active one signed out: nothing moves
        connect_as(&b, "default", "at-d2", "d@example.com").await;
        b.host.account_signout(FX, "google", &json!({"label": "work"})).await.unwrap();
        assert_eq!(pushed_labels(&b).await, ["google/default"]);
        b.host.account_signout(FX, "google", &json!({"label": "default"})).await.unwrap();
        assert!(pushed_labels(&b).await.is_empty() && b.host.active_label(FX, "google").is_none());
        b.host.stop_all().await;
        let _ = std::fs::remove_dir_all(&b.dir);
    }

    #[tokio::test]
    async fn a_plugin_that_declares_accounts_and_answers_32601_fails_plugin_invalid() {
        let b = bed_in("pkg32601", HostClock::manual()).await;
        publish_pkg(&b.files, &b.base, "v1.0.0", |p| p["mcpServers"]["fx"]["args"] = json!(["--misbehave=no-accounts"]));
        accounts_config(&b, &Arc::new(Mutex::new(Vec::new())));
        install_pkg(&b, "v1.0.0").await;
        google_client(&b);
        let (flow, state, _) = sign_in(&b).await;
        token_answer(&b.files, 200, json!({"access_token": "at-1", "refresh_token": "rt-1", "expires_in": 3600, "token_type": "Bearer"}));
        assert_eq!(b.host.account_callback(&flow, Some("c"), Some(&state), None).await.0, "connected");
        let rt = b.host.runtime_view(FX);
        assert_eq!((rt.state, rt.code.as_deref(), rt.message.as_str()), ("failed", Some("plugin_invalid"), "app.buwiz.fixture does not implement buwiz.account.set"));
        // and so does a spawn that pushes after provision
        assert_eq!(b.host.enable(FX).await.unwrap()["code"], "plugin_invalid");
        b.host.stop_all().await;
        let _ = std::fs::remove_dir_all(&b.dir);
    }

    #[tokio::test]
    async fn a_2_1_release_is_refused_when_a_file_or_the_served_manifest_is_not_the_packages() {
        let b = bed_in("pkgbad", HostClock::system()).await;
        let run = run_in(&b.ctx);
        let install = async |tag: &str| call(&run, "app_install", json!({"url": format!("https://github.com/o/r/releases/tag/{tag}")})).await;
        publish_pkg(&b.files, &b.base, "short", |p| p["buwiz"]["short"] = json!("fy"));
        let out = install("short").await;
        assert!(out.content.as_str().unwrap().contains("the served manifest's short is fx; plugin.json's buwiz.short is fy"), "{out:?}");
        publish_pkg(&b.files, &b.base, "badbin", |p| p["buwiz"]["binaries"][release::platform()]["sha256"] = json!("c".repeat(64)));
        let out = install("badbin").await;
        assert_eq!(out, Out::err("app_install: bin/fx does not match plugin.json's sha256".into()));
        publish_pkg(&b.files, &b.base, "noplat", |p| {
            let bin = p["buwiz"]["binaries"][release::platform()].clone();
            p["buwiz"]["binaries"] = json!({"plan9-mips": bin});
        });
        assert_eq!(install("noplat").await, Out::err(format!("app_install: no build for {}", release::platform())));
        // a plugin.json changed after signing
        publish_pkg(&b.files, &b.base, "tampered", |_| {});
        {
            let mut f = b.files.lock().unwrap();
            let e = f.get_mut("/obj/tampered/plugin.json").unwrap();
            let mut p: Value = serde_json::from_slice(&e.2).unwrap();
            p["mcpServers"]["fx"]["command"] = json!("/bin/sh");
            e.2 = p.to_string().into_bytes();
        }
        assert!(install("tampered").await.content.as_str().unwrap().contains("buwiz.signature does not verify"));
        assert!(b.host.row(FX).is_none(), "nothing installed");
        let _ = std::fs::remove_dir_all(&b.dir);
    }
}
