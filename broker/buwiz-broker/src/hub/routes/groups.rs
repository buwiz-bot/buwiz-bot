//! The sidebar's groups (5.2; routes/groups.mjs): the list with resolved
//! members, create, rename / recolor / fold / move, delete, and members
//! added, moved and removed (`library/groups.rs`).

use std::sync::Arc;

use serde_json::{json, Value};

use super::chat::{ok, origin_of, parse, ready, refused, thrown};
use crate::hub::ctx::HubCtx;
use crate::hub::library::groups;
use crate::hub::types::{HubRequest, HubResponse, Pattern::Exact as X, Verb};
use crate::hub::Registry;

pub fn install(routes: &mut Registry) {
    routes.route(Verb::Get, X("/groups"), list);
    routes.route(Verb::Post, X("/group/new"), |ctx, req| post(ctx, req, groups::new_group));
    routes.route(Verb::Post, X("/group/update"), |ctx, req| post(ctx, req, groups::update_group));
    routes.route(Verb::Post, X("/group/delete"), |ctx, req| post(ctx, req, groups::delete_group));
    routes.route(Verb::Post, X("/group/item"), |ctx, req| post(ctx, req, groups::group_item));
}

async fn list(ctx: Arc<HubCtx>, req: HubRequest) -> HubResponse {
    ready!(ctx);
    match groups::list(ctx.clone(), origin_of(&req)).await {
        Ok(v) => ok(&v),
        Err(e) => thrown(&e),
    }
}

/// `{ok: true, ...post(body)}`, or 400 with the error.
async fn post(ctx: Arc<HubCtx>, req: HubRequest, f: fn(&HubCtx, &Value) -> Result<Value, String>) -> HubResponse {
    ready!(ctx);
    let r = parse(&req.body).and_then(|b| f(&ctx, &b));
    match r {
        Ok(v) => {
            let mut o = json!({"ok": true});
            if let (Some(o), Some(v)) = (o.as_object_mut(), v.as_object()) {
                o.extend(v.clone());
            }
            ok(&o)
        }
        Err(e) => refused(&e, true),
    }
}
