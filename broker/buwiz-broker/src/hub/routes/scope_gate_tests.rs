//! The `@` menu's scopes follow the Bot: resolve and complete of a plugin's or an MCP server's
//! scope are refused when it is switched off for the Bot of the chat they are for, before the
//! plugin or server is asked anything. A real plugin host with the fixture plugin, a real MCP host
//! with the fixture server, and the hub's own routes.

use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::header::HeaderMap;
use hyper::Method;
use serde_json::{json, Value};

use crate::helper::keystore::Keystore;
use crate::helper::mcp_servers::McpHost;
use crate::helper::plugins::{Clock as HostClock, Host, Options, Timeouts};
use crate::hub::ctx::HubCtx;
use crate::hub::store::Bot;
use crate::hub::traits::Clock;
use crate::hub::types::HubRequest;
use crate::hub::Hub;
use crate::leaf::mcp::McpRoutes;

const FX: &str = "app.buwiz.scopefx";
const SEED: [u8; 32] = [7; 32];

struct Bed {
    hub: Hub,
    dir: PathBuf,
    token: String,
    /// the chats: a plain one, and one each for Ann and Bo
    plain: String,
    ann: String,
    bo: String,
}

fn node() -> String {
    std::env::var("PATH").ok().and_then(|p| p.split(':').map(|d| std::path::Path::new(d).join("node")).find(|p| p.exists())).map_or_else(|| "node".into(), |p| p.display().to_string())
}

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join(name).display().to_string()
}

async fn bed(label: &str) -> Bed {
    let dir = std::env::temp_dir().join(format!("buwiz-scope-gate-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("generated")).unwrap();
    let db = dir.join("generated").join("app.db");
    let mut ctx = HubCtx::new(dir.clone(), Clock::manual());
    ctx.store.load().unwrap();

    // the plugin: its one scope is resolved by `scopefx_find`, which writes a line per call
    let public = buwiz_protocol::keys::SessionKey::from_seed(&buwiz_protocol::keys::b64(&SEED)).unwrap().public();
    let trust = json!({"v": 1, "publishers": {"scopefx-test-01": {"publisher": "app.buwiz", "alg": "ed25519", "public_key": public}},
        "approved_plugins": [{"id": FX, "publisher_key_id": "scopefx-test-01"}]});
    std::fs::write(dir.join("trust.json"), trust.to_string()).unwrap();
    let host = Host::new(Options {
        db: db.clone(),
        keys: Keystore::from_spec(&format!("file:{}", dir.join("keys").display())).unwrap(),
        trust_file: Some(dir.join("trust.json")),
        log_dir: None,
        clock: HostClock::system(),
        timeouts: Timeouts::default(),
        apps_dir: None,
    })
    .unwrap();
    let manifest = json!({"protocol": "1.0.0", "id": FX, "short": "scopefx", "version": "1.0.0", "title": "Scope Fixture", "description": "A plugin with a scope.",
        "publisher": {"id": "app.buwiz", "key_id": "scopefx-test-01", "name": "Fixture"},
        "scopes": [{"name": "item", "description": "An item", "resolve": "scopefx_find"}],
        "apps": [{"id": "home", "title": "Home", "description": "An app to open.", "entry": "buwiz-app://home", "mime": "text/x-nsh-tsx", "sdk": "1",
            "fingerprint": format!("sha256:{}", "0".repeat(64)), "placement": ["full"], "permissions": {}}]});
    let signed = crate::v1::signing::sign_manifest(&manifest, &buwiz_protocol::keys::b64(&SEED), &[("home".to_owned(), b"x".to_vec())]).unwrap();
    std::fs::write(dir.join("manifest.json"), signed.to_string()).unwrap();
    let mut body = json!({"command": node(), "args": [fixture("fixture-plugin.mjs")], "env": {"FX_MANIFEST": dir.join("manifest.json"), "FX_CALLS": dir.join("plugin-calls")}});
    body["manifest_sha256"] = host.inspect(&body).await.unwrap()["manifest_sha256"].clone();
    host.install(&body).await.unwrap();
    for _ in 0..400 {
        if host.runtime_of(FX).0 == "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(host.runtime_of(FX).0, "running");
    ctx.sessions = Some(crate::helper::sessions::Sessions::new(host.clone()));
    ctx.plugins = Some(host);

    // the MCP server: its scope `item` is resolved by `resolve`, which writes a line per call
    let mcp = McpHost::new(&db, 8798).unwrap();
    ctx.db
        .with(|c| {
            c.execute(
                "INSERT INTO mcp_servers(id, name, command, args, env, enabled, created_at) VALUES('apps','Apps','node',?,?,1,1)",
                rusqlite::params![json!([fixture("fixtures/mcp-apps-server.mjs")]).to_string(), json!({"FIXTURE_CALLS": dir.join("mcp-calls").display().to_string(), "FIXTURE_MARK": dir.join("mcp-started").display().to_string()}).to_string()],
            )
        })
        .unwrap();
    ctx.mcp = Some(mcp);

    // two Bots, each with its own chat, and a plain chat
    let (plain, ann, bo) = ctx
        .store
        .write(|s| {
            let plain = s.list[0].id.clone();
            let mut chat = |name: &str, id: &str| {
                let t = s.fresh_thread(name);
                let tid = t.id.clone();
                s.list.push(t);
                s.bots.push(Bot::new(id, name, "blob", "#FFFFFF", Some(tid.clone())));
                tid
            };
            let (ann, bo) = (chat("Ann", "b_ann"), chat("Bo", "b_bo"));
            (plain, ann, bo)
        })
        .unwrap();
    let hub = Hub::with(ctx, vec![], |routes, ctx| crate::hub::routines::install(routes, ctx));
    hub.start();
    let token = std::fs::read_to_string(dir.join("host-token")).unwrap();
    Bed { hub, dir, token, plain, ann, bo }
}

impl Bed {
    fn switch(&self, owner: &str, key: &str, state: &str) {
        self.hub.ctx().db.with(|c| c.execute("INSERT OR REPLACE INTO bot_tools(bot_id, key, enabled, state, actor, at) VALUES(?,?,?,?,'human',1)", rusqlite::params![owner, key, i64::from(state != "off"), state])).unwrap();
    }

    async fn host(&self, method: Method, url: &str, body: Option<Value>) -> (u16, Value) {
        let mut headers = HeaderMap::new();
        headers.insert("x-buwiz-host", self.token.parse().unwrap());
        let body = body.map(|b| Bytes::from(b.to_string())).unwrap_or_default();
        let res = self.hub.answer(HubRequest { method, url: url.to_owned(), headers, body, params: Vec::new() }).await;
        let status = res.status().as_u16();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// `POST /v1/host/resolve` for the fixture plugin's scope, for `thread`.
    async fn resolve_v1(&self, thread: Option<&str>, complete: bool) -> (u16, Value) {
        let mut b = json!({"plugin": FX, "kind": "scope", "name": "item", "value": "x", "complete": complete, "context": {"attached": []}});
        if let Some(t) = thread {
            b["thread"] = json!(t);
        }
        self.host(Method::POST, "/v1/host/resolve", Some(b)).await
    }

    /// `/mcp/resolve` for the fixture server's scope, as the route's handler takes it.
    async fn resolve_mcp(&self, thread: Option<&str>, complete: bool) -> Value {
        let routes = McpRoutes::new(self.hub.ctx().mcp.clone().unwrap(), Vec::new()).with_gate(self.hub.ctx().clone());
        let mut b = json!({"server": "apps", "name": "item", "args": "x", "complete": complete});
        if let Some(t) = thread {
            b["thread"] = json!(t);
        }
        routes.resolve(&b).await
    }

    fn plugin_calls(&self) -> usize {
        std::fs::read_to_string(self.dir.join("plugin-calls")).map(|t| t.lines().count()).unwrap_or(0)
    }

    fn mcp_calls(&self) -> usize {
        std::fs::read_to_string(self.dir.join("mcp-calls")).map(|t| t.lines().count()).unwrap_or(0)
    }

    async fn done(self) {
        if let Some(h) = &self.hub.ctx().plugins {
            h.stop_all().await;
        }
        if let Some(m) = &self.hub.ctx().mcp {
            m.stop_all().await;
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn a_plugin_off_for_a_bot_is_refused_for_resolve_and_complete_and_never_called() {
    let b = bed("v1").await;
    let key = format!("v1:{FX}");
    b.switch("b_ann", &key, "off");
    for complete in [false, true] {
        let (status, v) = b.resolve_v1(Some(&b.ann), complete).await;
        assert_eq!((status, v["code"].as_str()), (409, Some("disabled")), "complete={complete}: {v}");
        assert_eq!(v["message"], "Scope Fixture is switched off for this Bot. Turn it on in Tools to use it here.");
    }
    assert_eq!(b.plugin_calls(), 0, "the plugin was not called");
    // another Bot, and plain chats, still have it
    for (chat, complete) in [(&b.bo, false), (&b.plain, true)] {
        let (status, v) = b.resolve_v1(Some(chat), complete).await;
        assert_eq!((status, v["result"].as_str()), (200, Some("choices")), "{v}");
    }
    assert_eq!(b.plugin_calls(), 2, "each answer is one call");
    // switched back on, Ann has it
    b.switch("b_ann", &key, "on");
    assert_eq!(b.resolve_v1(Some(&b.ann), false).await.0, 200);
    assert_eq!(b.plugin_calls(), 3);
    b.done().await;
}

#[tokio::test]
async fn a_plugin_off_for_plain_chats_is_refused_there_and_a_chat_must_be_named() {
    let b = bed("v1chats").await;
    b.switch("@chats", &format!("v1:{FX}"), "off");
    let (status, v) = b.resolve_v1(Some(&b.plain), false).await;
    assert_eq!((status, v["code"].as_str()), (409, Some("disabled")));
    assert_eq!(v["message"], "Scope Fixture is switched off for plain chats. Turn it on in Tools to use it here.");
    assert_eq!(b.resolve_v1(Some(&b.ann), false).await.0, 200, "a Bot's own switches are its own");
    let (status, v) = b.resolve_v1(None, false).await;
    assert_eq!((status, v["code"].as_str()), (400, Some("invalid_args")), "{v}");
    assert_eq!(b.plugin_calls(), 1, "only Ann's was answered");
    b.done().await;
}

#[tokio::test]
async fn an_mcp_server_off_for_a_bot_is_refused_for_resolve_and_complete_and_never_called() {
    let b = bed("mcp").await;
    b.switch("b_ann", "mcp:apps", "off");
    for complete in [false, true] {
        let v = b.resolve_mcp(Some(&b.ann), complete).await;
        assert_eq!((v["ok"].clone(), v["code"].as_str()), (json!(false), Some("disabled")), "complete={complete}: {v}");
        assert_eq!(v["error"], "Apps is switched off for this Bot. Turn it on in Tools to use it here.");
    }
    assert_eq!(b.mcp_calls(), 0, "the server was not called");
    assert!(!b.dir.join("mcp-started").exists(), "nor started");
    for (chat, complete) in [(&b.bo, false), (&b.plain, true)] {
        let v = b.resolve_mcp(Some(chat), complete).await;
        assert!(v.get("choices").is_some() || v["ok"] == json!(true), "{v}");
    }
    assert_eq!(b.mcp_calls(), 2);
    let v = b.resolve_mcp(None, false).await;
    assert_eq!((v["ok"].clone(), v["code"].as_str()), (json!(false), Some("invalid_args")), "{v}");
    assert_eq!(b.mcp_calls(), 2);
    b.done().await;
}

#[tokio::test]
async fn attaching_a_token_of_an_mcp_server_that_is_off_for_the_bot_is_refused() {
    let b = bed("attach").await;
    b.switch("b_ann", "mcp:apps", "off");
    let (_, v) = b.host_attach("@apps:item:x", &b.ann).await;
    assert_eq!(v["error"], "Apps is switched off for this Bot. Turn it on in Tools to use it here.", "{v}");
    assert_eq!(b.mcp_calls(), 0);
    let (_, v) = b.host_attach("@apps:item:x", &b.bo).await;
    assert!(v.get("attachment").is_some(), "{v}");
    b.done().await;
}

#[tokio::test]
async fn the_menu_asks_which_plugins_and_servers_are_off_for_the_chats_bot() {
    let b = bed("off").await;
    b.switch("b_ann", &format!("v1:{FX}"), "off");
    b.switch("b_ann", "mcp:apps", "off");
    b.switch("b_ann", "web", "off");
    b.switch("b_bo", "mcp:apps", "off");
    let (status, v) = b.off_for(&b.ann).await;
    assert_eq!((status, v["bot"].as_str(), v["off"].clone()), (200, Some("b_ann"), json!(["mcp:apps", format!("v1:{FX}")])), "only plugins and servers, not `web`");
    assert_eq!(b.off_for(&b.bo).await.1["off"], json!(["mcp:apps"]));
    let (_, v) = b.off_for(&b.plain).await;
    assert_eq!((v["bot"].as_str(), v["off"].clone()), (Some("@chats"), json!([])), "plain chats have their own");
    // the answer is the one the refusal reads: switching it back is seen at once
    b.switch("b_ann", "mcp:apps", "on");
    assert_eq!(b.off_for(&b.ann).await.1["off"], json!([format!("v1:{FX}")]));
    assert_eq!(b.host(Method::GET, "/v1/host/tools/off", None).await.0, 400, "a chat must be named");
    b.done().await;
}

impl Bed {
    /// `GET /v1/host/tools/off` for `thread`.
    async fn off_for(&self, thread: &str) -> (u16, Value) {
        self.host(Method::GET, &format!("/v1/host/tools/off?thread={thread}"), None).await
    }

    async fn host_attach(&self, token: &str, thread: &str) -> (u16, Value) {
        match crate::hub::library::attachments::attach(self.hub.ctx(), &json!({"token": token, "thread": thread})).await {
            Ok(v) => (200, v),
            Err(e) => (400, json!({"error": e})),
        }
    }
}
