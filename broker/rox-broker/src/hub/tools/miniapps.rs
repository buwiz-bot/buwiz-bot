//! The agent's mini app tools (drafts' install contract, part 2): Rust's
//! own, not in `defs.json` (which stays Node's, DEFS-01). `app_install`
//! fetches and verifies a GitHub release (`helper::release`), inspects it
//! and posts an `app_install` card in the chat: only the user's Install
//! installs it. `app_uninstall` posts an `app_uninstall` card the same way;
//! `app_enable` / `app_disable` act at once; `app_list` lists. A 2.1
//! package's card also names its folder (`package`), skills and accounts.

use serde_json::{json, Value};

use super::{Out, Run};
use crate::helper::plugins::{parse, Host};
use crate::helper::release;
use crate::hub::routines::store_host::StoreHost;
use crate::hub::routines::threads;
use crate::js;

pub const NAMES: [&str; 5] = ["app_install", "app_list", "app_enable", "app_disable", "app_uninstall"];

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
            "description": "Offer to install a mini app from its GitHub release: send the repository (https://github.com/<owner>/<repo>, optionally /releases/tag/<tag>, or <owner>/<repo>). The release is downloaded and verified against the trusted publishers, then a card in this chat asks the user to review it and press Install. You cannot install it yourself, and you never ask for or pass its settings or secrets: the card asks the user for them.",
            "input_schema": {"type": "object", "properties": {"url": {"type": "string", "description": "https://github.com/<owner>/<repo>[/releases/tag/<tag>] or <owner>/<repo>"}}, "required": ["url"]},
        }),
        json!({
            "name": "app_list",
            "description": "List the installed mini apps: id, title, version, state and whether each is enabled.",
            "input_schema": {"type": "object", "properties": {}},
        }),
        json!({"name": "app_enable", "description": "Switch an installed mini app on (it starts and is verified again).", "input_schema": id_schema("the one to switch on")}),
        json!({"name": "app_disable", "description": "Switch an installed mini app off: its process stops; its data and settings stay.", "input_schema": id_schema("the one to switch off")}),
        json!({
            "name": "app_uninstall",
            "description": "Ask the user to uninstall a mini app: a card in this chat asks them to confirm. You cannot uninstall it yourself.",
            "input_schema": id_schema("the one to remove"),
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

fn list(host: &Host) -> Out {
    let rows = host.rows();
    if rows.is_empty() {
        return Out::ok("no mini apps installed".into());
    }
    let lines: Vec<String> = rows
        .iter()
        .map(|r| {
            let state = host.runtime_view(&r.id).state;
            format!("{} — {} · {} · {state} · {}", r.id, title_of(&r.manifest(), &r.id), r.version, if r.enabled { "enabled" } else { "disabled" })
        })
        .collect();
    Out::ok(lines.join("\n"))
}

async fn install(run: &Run, host: &Host, input: &Value) -> Out {
    let url = input.get("url").filter(|v| !v.is_null()).map(js::string_of).unwrap_or_default();
    let src = match release::parse_url(&url) {
        Ok(s) => s,
        Err(e) => return Out::err(e),
    };
    run.set_job("connecting", "Fetching the mini app");
    let api = run.ctx.ext.get::<GithubApi>().map(|g| g.0.clone()).unwrap_or_else(|| release::GITHUB_API.to_owned());
    let f = match release::fetch(&api, &src, &host.trust(), &run.ctx.data).await {
        Ok(f) => f,
        Err(e) => return Out::err(format!("app_install: {e}")),
    };
    let mut spec = f.spec();
    if f.package.is_some() {
        spec["package"] = json!(f.dir.display().to_string());
    }
    let i = match host.inspect(&spec).await {
        Ok(i) => i,
        Err(e) => return Out::err(format!("app_install: the release did not pass inspection: {}: {}", e.code, e.message)),
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
    Out::ok(format!("a card in this chat asks the user to review and Install {title} {}; you cannot install it yourself", f.version))
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
        rox_v1::keys::sha256_hex(b)
    }

    /// The fixture plugin as a release: its signed manifest, a launcher
    /// script as the binary, the index; served under tag `tag`.
    fn publish(files: &Files, base: &str, tag: &str, bin_sha: Option<&str>) {
        let dir = server_dir();
        let mut m: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
        m["publisher"]["key_id"] = json!(KEY);
        let sources: Vec<(String, Vec<u8>)> = ["board", "card"].iter().map(|id| (id.to_string(), std::fs::read(dir.join(format!("{id}.tsx"))).unwrap())).collect();
        let manifest = crate::v1::signing::sign_manifest(&m, &rox_v1::keys::b64(&SEED), &sources).unwrap().to_string().into_bytes();
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
        assert_eq!(none, Out::err("app_install: o/nothing has no published release".into()));

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

    /// The fixture as a 2.1 package: plugin.json (signed, with a google
    /// account), the served manifest and a skill as files, the launcher
    /// as this platform's binary; `edit` changes plugin.json before it is
    /// signed.
    fn publish_pkg(files: &Files, base: &str, tag: &str, edit: impl FnOnce(&mut Value)) {
        let dir = server_dir();
        let mut m: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
        m["publisher"]["key_id"] = json!(KEY);
        let sources: Vec<(String, Vec<u8>)> = ["board", "card"].iter().map(|id| (id.to_string(), std::fs::read(dir.join(format!("{id}.tsx"))).unwrap())).collect();
        let manifest = crate::v1::signing::sign_manifest(&m, &rox_v1::keys::b64(&SEED), &sources).unwrap().to_string().into_bytes();
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
        assert_eq!(rox_v1::keys::sha256_hex(form["code_verifier"].as_bytes()).len(), 64);
        let tok = |t: &str| rox_v1::keys::sha256_hex(t.as_bytes())[..12].to_owned();
        assert_eq!(pushed(&b.host).await, json!({"google/default": {"subject": "u@example.com", "expires_at": 3600, "token": tok("at-1"), "sets": 1}}));
        let rows = b.host.account_rows(FX);
        assert_eq!((rows[0].state.as_str(), rows[0].subject.as_str(), rows[0].token_ref.as_str()), ("connected", "u@example.com", "app.buwiz.fixture/google/default"));
        assert_eq!(rows[0].scopes, ["https://www.googleapis.com/auth/gmail.readonly", "openid", "email"]);
        let keys = Keystore::from_spec(&format!("file:{}", b.dir.join("keys").display())).unwrap();
        assert_eq!(keys.get_token("app.buwiz.fixture/google/default").unwrap().unwrap()["refresh_token"], "rt-1");
        // the views carry no token
        let listed = crate::hub::routines::plugins::describe(&b.host, &b.host.row(FX).unwrap(), &|_, _| json!({}));
        assert_eq!(listed["accounts"], json!([{"account": "google", "label": "default", "provider": "google", "title": "Google account", "subject": "u@example.com", "state": "connected", "required": true}]));
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
