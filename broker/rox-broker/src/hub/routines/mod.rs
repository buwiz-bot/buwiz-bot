//! Routines (plan 5.5, track D): v0 routines on a timer (`v0`, the
//! templates servers announce in `templates`), v1 plugin routine instances
//! (`v1`, their threads in `threads`), the agent's routine tools (`tools`),
//! and what they need from the rest of the hub as seams: the store
//! (`host::RoutineHost`), the v1 plugin host (`plugins::PluginView`, with
//! the plugin list of GET /v1/host/plugins) and the MCP servers
//! (`mcp::McpView`). Turns run through the skeleton's `TurnRunner`.
//!
//! Owns the tables `routines`, `v1_routines`, `v1_instances` and the `meta`
//! keys `google_email`, `google_email_seen`. The routes are
//! `hub/routes/routines.rs` (v0) and `hub/routes/host.rs` (5.5h: every
//! `/v1/host/*` route, the `ri_` webhooks and the host token).

pub mod host;
pub mod mcp;
pub mod plugins;
pub mod templates;
pub mod text;
pub mod threads;
pub mod tools;
pub mod v0;
pub mod v1;

#[cfg(test)]
mod tests;

use std::sync::{Arc, Mutex};

use serde_json::Value;

use self::host::{RoutineHost, Unstored};
use self::mcp::{HostMcp, McpView, NoMcp};
use self::plugins::{NoPlugins, PluginView};
use self::v0::V0;
use self::v1::V1;
use super::ctx::HubCtx;
use super::types::TurnCtx;
use super::Registry;

pub struct Routines {
    pub host: Arc<dyn RoutineHost>,
    pub v0: Arc<V0>,
    pub v1: Arc<V1>,
    /// Rox's host token, made at start (`hub/routes/host.rs`)
    pub token: Mutex<Option<String>>,
    /// whether the schedulers run at start (not without a store)
    pub scheduled: bool,
}

impl Routines {
    pub fn new(ctx: &HubCtx, host: Arc<dyn RoutineHost>, plugins: Arc<dyn PluginView>, mcp: Arc<dyn McpView>, scheduled: bool) -> Arc<Routines> {
        let v0 = V0::new(&ctx.db, host.clone(), mcp);
        let v1 = V1::new(ctx, plugins, host.clone());
        Arc::new(Routines { host, v0, v1, token: Mutex::new(None), scheduled })
    }

    /// The hub's start, as app-server.mjs's listen callback: the host
    /// token, the v0 timers, the v1 plugins, the v1 poll.
    pub fn start(self: &Arc<Self>, ctx: &Arc<HubCtx>) {
        super::routes::host::init_token(self, &ctx.data);
        if self.scheduled {
            self.v0.start(ctx);
        }
        if let Some(host) = ctx.plugins.clone() {
            tokio::spawn(async move { host.start_all().await });
        }
        self.v1.listen(ctx);
        if self.scheduled {
            self.v1.start(ctx);
        }
    }
}

/// The routines on the hub's plugin and MCP hosts, with no store yet
/// ([`Unstored`]): every route answers, no chat exists, and the
/// schedulers stay off, so a real data dir's routines are never ticked
/// (and switched off as "its chat was deleted") before the store is in.
pub fn install(routes: &mut Registry, ctx: &mut HubCtx) {
    install_with(routes, ctx, Arc::new(Unstored), false);
}

/// The routines on the store `host`; `scheduled`: their timers and poll
/// run from the hub's start.
pub fn install_with(routes: &mut Registry, ctx: &mut HubCtx, host: Arc<dyn RoutineHost>, scheduled: bool) -> Arc<Routines> {
    let plugins: Arc<dyn PluginView> = match &ctx.plugins {
        Some(h) => Arc::new(h.clone()),
        None => Arc::new(NoPlugins),
    };
    let mcp: Arc<dyn McpView> = match &ctx.mcp {
        Some(m) => Arc::new(HostMcp(m.clone())),
        None => Arc::new(NoMcp),
    };
    let rs = Routines::new(ctx, host, plugins, mcp, scheduled);
    mount(routes, ctx, rs.clone());
    rs
}

/// Registers the routines' routes, webhooks, tools and start.
pub fn mount(routes: &mut Registry, ctx: &mut HubCtx, rs: Arc<Routines>) {
    super::routes::routines::install(routes, rs.clone());
    super::routes::host::install(routes, rs.clone());
    for name in tools::NAMES {
        let rs = rs.clone();
        ctx.tools.register(name, move |c: Arc<HubCtx>, turn: TurnCtx, input: Value| {
            let rs = rs.clone();
            async move { tools::call(rs, c, turn, name, input).await }
        });
    }
    routes.on_start(move |ctx| rs.start(&ctx));
}
