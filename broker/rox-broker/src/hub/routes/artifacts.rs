//! The Library's per-artifact routes (5.7b; routes/artifacts.mjs):
//! details, icons, hiding and deleting for images and files, a mini app's
//! details passed through to `/app/meta`'s edit, a Drive file's rename sent
//! to Drive.

use std::sync::Arc;

use serde_json::{json, Value};

use super::apps::{body, fail, get, icon, json_at, str_or};
use super::chat::origin_of;
use crate::hub::ctx::HubCtx;
use crate::hub::library::appmeta::str_or_empty;
use crate::hub::library::artifacts::{art_icon_path, delete_artifact, hide_artifact, info, rename_remote, update_artifact_meta};
use crate::hub::types::{HubRequest, HubResponse, Pattern::Exact as X, Verb};
use crate::hub::Registry;

pub fn install(routes: &mut Registry) {
    routes.route(Verb::Post, X("/artifact/meta"), meta);
    routes.route(Verb::Post, X("/artifact/rename-remote"), rename);
    routes.route(Verb::Get, X("/artifact/info"), info_route);
    routes.route(Verb::Post, X("/artifact/hide"), hide);
    routes.route(Verb::Post, X("/artifact/delete"), delete);
    routes.route(Verb::Get, X("/artifact/icon"), icon_route);
}

fn answered(r: Result<Value, String>, status: u16) -> HubResponse {
    match r {
        Ok(v) => json_at(200, &v, true),
        Err(e) => fail(status, &e, true),
    }
}

/// `POST /artifact/meta {id, title?, filename?, description?, icon?}`:
/// answers the artifact's info.
async fn meta(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let r = async {
        let j = body(&req)?;
        let id = str_or(get(&j, "id"));
        update_artifact_meta(&ctx, &id, &j).await?;
        info(&ctx, &id, &origin_of(&req))
    }
    .await;
    answered(r, 400)
}

/// `POST /artifact/rename-remote {id: gdrive:<file id>, name}`.
async fn rename(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let r = async {
        let j = body(&req)?;
        rename_remote(&ctx, &str_or(get(&j, "id")), &str_or_empty(get(&j, "name")), &origin_of(&req)).await
    }
    .await;
    answered(r, 400)
}

/// `GET /artifact/info?id=`: everything Edit Details shows.
async fn info_route(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let id = req.query("id").unwrap_or_default();
    answered(info(&ctx, &id, &origin_of(&req)), 404)
}

/// `POST /artifact/hide {id, hidden = true}`.
async fn hide(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let r = (|| -> Result<_, String> {
        let j = body(&req)?;
        hide_artifact(&ctx, &str_or(get(&j, "id")), get(&j, "hidden") != Some(&Value::Bool(false)))?;
        Ok(json!({"ok": true}))
    })();
    answered(r, 400)
}

/// `POST /artifact/delete {id: file:<path>}`.
async fn delete(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let r = (|| -> Result<_, String> {
        let j = body(&req)?;
        delete_artifact(&ctx, &str_or(get(&j, "id")))?;
        Ok(json!({"ok": true}))
    })();
    answered(r, 400)
}

/// `GET /artifact/icon?id=…`: an image's or file's icon, 404 with none.
async fn icon_route(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    let id = req.query("id").unwrap_or_default();
    let p = (!id.is_empty()).then(|| art_icon_path(&ctx, &id)).filter(|p| p.exists());
    icon(p.as_deref())
}
