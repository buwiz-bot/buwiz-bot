//! Narrow read-only external-app door (`POST /v1/ext/read`) and its host-gated
//! grant management (`/v1/host/ext/grant`, `/v1/host/ext/grants`, `/v1/host/ext/revoke`).
//!
//! Designed for external local desktop apps (such as Buwiz Forms) that need
//! read-only data from an installed v1 mini app (such as Buwiz Books) without
//! opening that mini app's SQLite database or routing private data through the cloud:
//! - Loopback-only (enforced by the broker's listener and Origin middleware).
//! - Bearer-token authenticated: each grant is scoped to one `client`, one `plugin`,
//!   and an explicit allow-list of **read-only** tools (`declared_read(&t) == true`).
//! - Token hashes are stored as SHA-256 hex in `<data dir>/ext-grants.json` (0600);
//!   the raw bearer token is returned only once on creation and never logged or re-read.
//! - Defense in depth: on every `/v1/ext/read` call, the broker re-checks that the
//!   plugin is installed, enabled and running, the tool is not disabled, AND the tool's
//!   live descriptor still declares `readOnlyHint: true`. Write tools are always refused.

use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use buwiz_protocol::keys::{b64, random};
use buwiz_protocol::proof::same;
use hyper::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::helper::plugins::result_data;
use crate::helper::sessions::result_bytes;
use crate::hub::ctx::HubCtx;
use crate::hub::policy::keys::declared_read;
use crate::hub::routines::plugins::disabled_of;
use crate::hub::tools::v1::{contributing, view};
use crate::hub::types::{self, HubRequest, HubResponse, Pattern, Verb};
use crate::hub::Registry;
use crate::js;
use crate::v1::V1Error;

const GRANTS_FILE: &str = "ext-grants.json";
const ARGS_MAX: usize = 256 * 1024;
const RESULT_MAX: usize = 1 << 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GrantRecord {
    id: String,
    client: String,
    plugin: String,
    tools: Vec<String>,
    token_sha256: String,
    created_at: u64,
}

fn file_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn grants_path(ctx: &HubCtx) -> PathBuf {
    ctx.data.join(GRANTS_FILE)
}

fn load_grants(ctx: &HubCtx) -> Vec<GrantRecord> {
    let path = grants_path(ctx);
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save_grants(ctx: &HubCtx, grants: &[GrantRecord]) -> Result<(), V1Error> {
    let path = grants_path(ctx);
    let tmp = ctx.data.join(format!("{GRANTS_FILE}.tmp.{}", std::process::id()));
    let bytes = serde_json::to_vec_pretty(grants)
        .map_err(|e| V1Error::new("internal", format!("serialize grants: {e}")))?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    opts.mode(0o600);
    let mut f = opts
        .open(&tmp)
        .map_err(|e| V1Error::new("internal", format!("open grants tmp: {e}")))?;
    f.write_all(&bytes)
        .and_then(|_| f.sync_all())
        .map_err(|e| V1Error::new("internal", format!("write grants tmp: {e}")))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| V1Error::new("internal", format!("rename grants: {e}")))?;
    Ok(())
}

fn sha256_hex(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

fn valid_slug(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Returns whether `tool` is currently exposed by running plugin `plugin`, not disabled,
/// and declared read-only (`annotations.readOnlyHint == true`).
fn is_live_read_only_tool(ctx: &HubCtx, plugin: &str, tool: &str) -> Result<(), V1Error> {
    let p = view(ctx);
    let Some(row) = contributing(p.as_ref()).into_iter().find(|r| r.id == plugin) else {
        return V1Error::err("plugin_unavailable", format!("plugin `{plugin}` is not running"));
    };
    let off = disabled_of(&row);
    if off.iter().any(|o| o == tool) {
        return V1Error::err("forbidden", format!("tool `{tool}` is disabled on `{plugin}`"));
    }
    let runtime = p.runtime(&row.id);
    let Some(desc) = runtime
        .tools
        .iter()
        .find(|t| t.get("name").and_then(Value::as_str) == Some(tool))
    else {
        return V1Error::err("not_found", format!("tool `{tool}` is not offered by `{plugin}`"));
    };
    if crate::v1::is_v1("reservedTool", desc.get("name")) || !declared_read(desc) {
        return V1Error::err(
            "forbidden",
            format!("tool `{tool}` is not a read-only tool and cannot be granted to /v1/ext/read"),
        );
    }
    Ok(())
}

/// `POST /v1/host/ext/grant { client, plugin, tools }` (host-token gated).
pub fn host_grant(ctx: &HubCtx, b: &Value) -> Result<Value, V1Error> {
    let client = b
        .get("client")
        .and_then(Value::as_str)
        .filter(|s| valid_slug(s))
        .ok_or_else(|| V1Error::new("invalid_args", "`client` must be a non-empty slug (1..=64 chars)"))?;
    let plugin = b
        .get("plugin")
        .and_then(Value::as_str)
        .filter(|s| valid_slug(s))
        .ok_or_else(|| V1Error::new("invalid_args", "`plugin` must be a valid plugin id"))?;
    let raw_tools = b
        .get("tools")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| V1Error::new("invalid_args", "`tools` must be a non-empty array of read-only tool names"))?;
    let mut tools = Vec::with_capacity(raw_tools.len());
    for t in raw_tools {
        let name = t
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .ok_or_else(|| V1Error::new("invalid_args", "every entry in `tools` must be a tool name"))?;
        is_live_read_only_tool(ctx, plugin, name)?;
        if !tools.iter().any(|existing: &String| existing == name) {
            tools.push(name.to_owned());
        }
    }

    let raw_token = format!("ext_{}", b64(&random::<32>()));
    let id = format!("g_{}", &sha256_hex(&random::<16>())[..16]);
    let record = GrantRecord {
        id: id.clone(),
        client: client.to_owned(),
        plugin: plugin.to_owned(),
        tools: tools.clone(),
        token_sha256: sha256_hex(raw_token.as_bytes()),
        created_at: (ctx.clock.now() as u64) / 1000,
    };

    let _guard = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut grants = load_grants(ctx);
    grants.push(record);
    save_grants(ctx, &grants)?;

    Ok(json!({
        "ok": true,
        "id": id,
        "client": client,
        "plugin": plugin,
        "tools": tools,
        "token": raw_token,
    }))
}

/// `GET /v1/host/ext/grants` (host-token gated; never returns raw tokens or hashes).
pub fn host_grants(ctx: &HubCtx) -> Result<Value, V1Error> {
    let _guard = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    let grants: Vec<Value> = load_grants(ctx)
        .into_iter()
        .map(|g| {
            json!({
                "id": g.id,
                "client": g.client,
                "plugin": g.plugin,
                "tools": g.tools,
                "created_at": g.created_at,
            })
        })
        .collect();
    Ok(json!({ "ok": true, "grants": grants }))
}

/// `POST /v1/host/ext/revoke { id }` (host-token gated).
pub fn host_revoke(ctx: &HubCtx, b: &Value) -> Result<Value, V1Error> {
    let id = b
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| V1Error::new("invalid_args", "send {id}"))?;
    let _guard = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut grants = load_grants(ctx);
    let before = grants.len();
    grants.retain(|g| g.id != id);
    let revoked = grants.len() < before;
    if revoked {
        save_grants(ctx, &grants)?;
    }
    Ok(json!({ "ok": true, "revoked": revoked }))
}

fn bearer_of(req: &HubRequest) -> Option<&str> {
    let raw = req.headers.get("authorization").and_then(|v| v.to_str().ok())?;
    let token = raw.strip_prefix("Bearer ").or_else(|| raw.strip_prefix("bearer "))?;
    let trimmed = token.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn find_grant(ctx: &HubCtx, token: &str) -> Option<GrantRecord> {
    let hash = sha256_hex(token.as_bytes());
    let _guard = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    load_grants(ctx)
        .into_iter()
        .find(|g| same(g.token_sha256.as_bytes(), hash.as_bytes()))
}

fn err_resp(status: StatusCode, code: &str, message: impl Into<String>) -> HubResponse {
    types::json(
        status,
        &json!({
            "ok": false,
            "code": code,
            "message": message.into(),
            "retryable": false,
        }),
    )
}

/// `POST /v1/ext/read { plugin, tool, args? }` — bearer-token authenticated read-only plugin call.
async fn ext_read(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let Some(token) = bearer_of(&req) else {
        return err_resp(StatusCode::UNAUTHORIZED, "unauthorized", "Authorization: Bearer <token> is required");
    };
    let Some(grant) = find_grant(&ctx, token) else {
        return err_resp(StatusCode::UNAUTHORIZED, "unauthorized", "invalid or revoked external read grant");
    };
    if req.body.len() > ARGS_MAX {
        return err_resp(StatusCode::PAYLOAD_TOO_LARGE, "invalid_args", "request body exceeds 256 KiB");
    }
    let Ok(body) = serde_json::from_slice::<Value>(&req.body) else {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_args", "body must be a JSON object {plugin, tool, args?}");
    };
    let Some(plugin) = body.get("plugin").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_args", "`plugin` is required");
    };
    let Some(tool) = body.get("tool").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
        return err_resp(StatusCode::BAD_REQUEST, "invalid_args", "`tool` is required");
    };
    let args = match body.get("args") {
        None | Some(Value::Null) => json!({}),
        Some(a) if a.is_object() => a.clone(),
        Some(_) => return err_resp(StatusCode::BAD_REQUEST, "invalid_args", "`args` must be a JSON object"),
    };

    if grant.plugin != plugin || !grant.tools.iter().any(|t| t == tool) {
        return err_resp(
            StatusCode::FORBIDDEN,
            "forbidden",
            format!("grant `{}` does not allow `{plugin}` / `{tool}`", grant.id),
        );
    }
    if let Err(e) = is_live_read_only_tool(&ctx, plugin, tool) {
        let status = match e.code.as_str() {
            "forbidden" => StatusCode::FORBIDDEN,
            "not_found" => StatusCode::NOT_FOUND,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        };
        return err_resp(status, &e.code, e.message);
    }

    let ts = crate::hub::policy::now(&ctx);
    let meta = json!({
        "buwiz/caller": {
            "origin": "agent",
            "plugin": plugin,
            "ts": ts,
        }
    });
    let r = match view(&ctx).call_tool(plugin, tool, args, Some(meta)).await {
        Ok(r) => r,
        Err(e) => return err_resp(StatusCode::BAD_GATEWAY, "plugin_error", e.message),
    };
    if r.get("isError").is_some_and(js::truthy) {
        let msg = r
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|c| c.get("type") == Some(&json!("text")))
            .and_then(|c| c.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("plugin tool returned an error");
        return err_resp(StatusCode::BAD_REQUEST, "tool_error", msg);
    }
    if result_bytes(&r) > RESULT_MAX {
        return err_resp(StatusCode::BAD_GATEWAY, "result_too_large", "tool result exceeds 1 MiB");
    }
    types::json(
        StatusCode::OK,
        &json!({
            "ok": true,
            "plugin": plugin,
            "tool": tool,
            "result": result_data(&r),
        }),
    )
}

pub fn install(routes: &mut Registry, _ctx: &mut HubCtx) {
    routes.route(Verb::Post, Pattern::Exact("/v1/ext/read"), |ctx, req| {
        Box::pin(ext_read(ctx, req))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use hyper::header::{HeaderMap, HeaderValue};
    use crate::hub::traits::Clock;

    fn temp_ctx(label: &str) -> HubCtx {
        let dir = std::env::temp_dir().join(format!(
            "buwiz-ext-read-{label}-{}-{}",
            std::process::id(),
            crate::hub::store::now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        HubCtx::new(dir, Clock::manual())
    }

    #[test]
    fn valid_slug_checks() {
        assert!(valid_slug("buwiz-forms"));
        assert!(valid_slug("app.buwiz.books"));
        assert!(!valid_slug(""));
        assert!(!valid_slug("bad/slug"));
        assert!(!valid_slug("bad slug"));
    }

    #[test]
    fn grant_store_roundtrip_and_revoke() {
        let ctx = temp_ctx("roundtrip");
        let raw_token = "ext_test_token_123456";
        let rec = GrantRecord {
            id: "g_001".to_owned(),
            client: "buwiz-forms".to_owned(),
            plugin: "app.buwiz.books".to_owned(),
            tools: vec!["books_accounts_list".to_owned()],
            token_sha256: sha256_hex(raw_token.as_bytes()),
            created_at: 1000,
        };
        save_grants(&ctx, &[rec]).unwrap();
        assert!(find_grant(&ctx, raw_token).is_some());
        assert!(find_grant(&ctx, "ext_wrong").is_none());

        // host_grants never leaks token or token_sha256
        let listed = host_grants(&ctx).unwrap();
        let text = listed.to_string();
        assert!(!text.contains(raw_token));
        assert!(!text.contains("token_sha256"));
        assert_eq!(listed["grants"].as_array().unwrap().len(), 1);

        // revoke removes it
        let revoked = host_revoke(&ctx, &json!({ "id": "g_001" })).unwrap();
        assert_eq!(revoked["revoked"], json!(true));
        assert!(find_grant(&ctx, raw_token).is_none());
    }

    #[tokio::test]
    async fn ext_read_rejects_missing_and_invalid_bearer() {
        let ctx = Arc::new(temp_ctx("unauth"));
        let req = HubRequest {
            method: hyper::Method::POST,
            url: "/v1/ext/read".to_owned(),
            headers: HeaderMap::new(),
            body: Bytes::from_static(br#"{"plugin":"app.buwiz.books","tool":"books_accounts_list"}"#),
            params: Vec::new(),
        };
        let resp = ext_read(Arc::clone(&ctx), req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer ext_not_valid"));
        let req2 = HubRequest {
            method: hyper::Method::POST,
            url: "/v1/ext/read".to_owned(),
            headers,
            body: Bytes::from_static(br#"{"plugin":"app.buwiz.books","tool":"books_accounts_list"}"#),
            params: Vec::new(),
        };
        let resp2 = ext_read(ctx, req2).await;
        assert_eq!(resp2.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn ext_read_rejects_ungranted_tool_or_plugin() {
        let ctx = Arc::new(temp_ctx("scoped"));
        let raw_token = "ext_scoped_token";
        save_grants(
            &ctx,
            &[GrantRecord {
                id: "g_scoped".to_owned(),
                client: "buwiz-forms".to_owned(),
                plugin: "app.buwiz.books".to_owned(),
                tools: vec!["books_accounts_list".to_owned()],
                token_sha256: sha256_hex(raw_token.as_bytes()),
                created_at: 1000,
            }],
        )
        .unwrap();

        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {raw_token}")).unwrap(),
        );
        let req = HubRequest {
            method: hyper::Method::POST,
            url: "/v1/ext/read".to_owned(),
            headers,
            body: Bytes::from_static(br#"{"plugin":"app.buwiz.books","tool":"books_app_post_entry"}"#),
            params: Vec::new(),
        };
        let resp = ext_read(ctx, req).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    struct FakeBooksPlugin {
        rows: Vec<crate::helper::plugins::Row>,
        tools: Vec<Value>,
        trust: crate::v1::trust::Trust,
    }

    impl crate::hub::routines::plugins::PluginView for FakeBooksPlugin {
        fn row(&self, id: &str) -> Option<crate::helper::plugins::Row> {
            self.rows.iter().find(|r| r.id == id).cloned()
        }
        fn rows(&self) -> Vec<crate::helper::plugins::Row> {
            self.rows.clone()
        }
        fn runtime(&self, id: &str) -> crate::helper::plugins::RuntimeView {
            if self.rows.iter().any(|r| r.id == id) {
                crate::helper::plugins::RuntimeView {
                    state: "running",
                    conn: true,
                    tools: self.tools.clone(),
                    ..crate::helper::plugins::RuntimeView::default()
                }
            } else {
                crate::helper::plugins::RuntimeView::default()
            }
        }
        fn trust(&self) -> crate::v1::trust::Trust {
            self.trust.clone()
        }
        fn call_tool(
            &self,
            _id: &str,
            tool: &str,
            _args: Value,
            _meta: Option<Value>,
        ) -> crate::hub::types::BoxFuture<'static, Result<Value, V1Error>> {
            let tool = tool.to_owned();
            Box::pin(async move {
                Ok(json!({
                    "content": [{ "type": "text", "text": format!(r#"{{"tool":"{tool}","accounts":[{{"code":"41000","name":"Sales Revenue"}}]}}"#) }],
                    "structuredContent": { "tool": tool, "accounts": [{ "code": "41000", "name": "Sales Revenue" }] }
                }))
            })
        }
        fn events(&self) -> Option<tokio::sync::broadcast::Receiver<Value>> {
            None
        }
    }

    #[tokio::test]
    async fn full_grant_read_only_and_revoke_lifecycle() {
        let mut ctx = temp_ctx("lifecycle");
        let manifest = json!({
            "id": "app.buwiz.books",
            "short": "books",
            "title": "Books",
            "publisher": { "id": "app.buwiz" }
        });
        let row = crate::helper::plugins::Row {
            id: "app.buwiz.books".to_owned(),
            short: "books".to_owned(),
            command: "books-mini-app".to_owned(),
            args: "[]".to_owned(),
            env: "{}".to_owned(),
            version: "0.9.14".to_owned(),
            protocol: "1.0.0".to_owned(),
            publisher_key_id: "key-1".to_owned(),
            plugin_pub: "pub".to_owned(),
            manifest: manifest.to_string(),
            manifest_sha256: "0".repeat(64),
            tools: "[]".to_owned(),
            enabled: true,
            disabled_tools: "[]".to_owned(),
            created_at: 1,
            package: None,
            skills: "[]".to_owned(),
            source: None,
        };
        let trust = crate::v1::trust::Trust {
            approved: vec![("app.buwiz.books".into(), "key-1".into())],
            ..crate::v1::trust::Trust::default()
        };
        let fake = FakeBooksPlugin {
            rows: vec![row],
            tools: vec![
                json!({
                    "name": "books_accounts_list",
                    "description": "List accounts",
                    "annotations": { "readOnlyHint": true }
                }),
                json!({
                    "name": "books_app_post_entry",
                    "description": "Post entry",
                    "annotations": { "readOnlyHint": false }
                }),
            ],
            trust,
        };
        ctx.ext
            .insert(crate::hub::tools::v1::PluginsSeam(Arc::new(fake)));
        let ctx = Arc::new(ctx);

        // 1. Write tool must be REFUSED at grant time
        let write_err = host_grant(
            &ctx,
            &json!({
                "client": "buwiz-forms",
                "plugin": "app.buwiz.books",
                "tools": ["books_app_post_entry"]
            }),
        )
        .unwrap_err();
        assert_eq!(write_err.code, "forbidden");

        // 2. Read-only tool succeeds and issues a one-time bearer token
        let granted = host_grant(
            &ctx,
            &json!({
                "client": "buwiz-forms",
                "plugin": "app.buwiz.books",
                "tools": ["books_accounts_list"]
            }),
        )
        .unwrap();
        let token = granted["token"].as_str().unwrap().to_owned();
        let grant_id = granted["id"].as_str().unwrap().to_owned();
        assert!(token.starts_with("ext_"));

        // 3. /v1/ext/read succeeds with the bearer token
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let resp = ext_read(
            Arc::clone(&ctx),
            HubRequest {
                method: hyper::Method::POST,
                url: "/v1/ext/read".to_owned(),
                headers: headers.clone(),
                body: Bytes::from_static(
                    br#"{"plugin":"app.buwiz.books","tool":"books_accounts_list","args":{}}"#,
                ),
                params: Vec::new(),
            },
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        use http_body_util::BodyExt;
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["result"]["accounts"][0]["code"], json!("41000"));

        // 4. After revoke, the same bearer token is rejected with 401
        let revoked = host_revoke(&ctx, &json!({ "id": grant_id })).unwrap();
        assert_eq!(revoked["revoked"], json!(true));
        let resp_after = ext_read(
            ctx,
            HubRequest {
                method: hyper::Method::POST,
                url: "/v1/ext/read".to_owned(),
                headers,
                body: Bytes::from_static(
                    br#"{"plugin":"app.buwiz.books","tool":"books_accounts_list","args":{}}"#,
                ),
                params: Vec::new(),
            },
        )
        .await;
        assert_eq!(resp_after.status(), StatusCode::UNAUTHORIZED);
    }
}
