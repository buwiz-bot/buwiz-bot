//! The model's tools (plan 5.4; tools.mjs's tool-loop half, v1/agent.mjs,
//! drive.mjs): the surface a turn offers (`tool_list`, Node's `toolList`)
//! and the dispatcher (`exec_tool`, `execTool`). The schemas and the system
//! prompt are Node's, dumped (`defs`). The bot, skill, routine and agent
//! tools are other tracks' handlers in `ctx.tools`; this module owns only
//! their dispatch.
//!
//! A turn's run is a [`Run`]: what Node keeps on the job object and in
//! AsyncLocalStorage (`modelCtx`, `actingBot`), passed explicitly: the
//! thread, the job, the acting bot, the model choice, a routine's surface
//! (`TurnCtx`), and the run's own state (`pendingApp`, `pendingOffice`,
//! `revise`, `linkTurn`, `cards`, `v1Cards`, `driveSeen`). The job's
//! building placeholder and revise cover are `Job::turn` / `Job::revising`.

pub mod activity;
pub mod artifact;
pub mod builtin;
pub mod defs;
pub mod drive;
pub mod mcp;
pub mod office;
pub mod v1;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{json, Map, Value};

use crate::hub::ctx::HubCtx;
use crate::hub::jobs::JobId;
use crate::hub::library::botctx;
use crate::hub::model::choice::label_of;
use crate::hub::model::Settings;
use crate::hub::types::TurnCtx;
use crate::js;

/// A tool's answer as the model gets it: `content`, then `is_error` only
/// where Node's result object has the key (`{content}` vs
/// `{content, is_error: false}` are different bytes on the wire).
#[derive(Clone, Debug, PartialEq)]
pub struct Out {
    pub content: Value,
    pub is_error: Option<bool>,
}

impl Out {
    pub fn ok(s: String) -> Self {
        Self { content: json!(s), is_error: None }
    }

    pub fn err(s: String) -> Self {
        Self { content: json!(s), is_error: Some(true) }
    }

    /// `{type: "tool_result", tool_use_id, ...r}`.
    pub fn result_block(&self, id: &Value) -> Value {
        let mut m = Map::new();
        m.insert("type".into(), json!("tool_result"));
        m.insert("tool_use_id".into(), id.clone());
        m.insert("content".into(), self.content.clone());
        if let Some(e) = self.is_error {
            m.insert("is_error".into(), json!(e));
        }
        Value::Object(m)
    }
}

/// `job.linkTurn`: what the tools leave for the reply's turn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkTurn {
    pub links: Vec<Value>,
    pub suggest: Option<Vec<Value>>,
}

/// The run's own fields of Node's job object.
#[derive(Default)]
pub struct RunState {
    /// the generate_app placeholder drawn while its input streams (a turn id)
    pub pending_app: Option<String>,
    /// office_create's
    pub pending_office: Option<String>,
    /// `job.revise`: a `module://` attachment's card (name, base_v)
    pub revise: Option<(String, Option<i64>)>,
    pub link_turn: Option<LinkTurn>,
    pub cards: u64,
    pub v1_cards: Vec<Value>,
    pub drive_seen: Vec<(String, Value)>,
}

/// One turn's run (Node's job, `modelCtx` and `actingBot`).
pub struct Run {
    pub ctx: Arc<HubCtx>,
    /// the thread, the job, the acting bot, the model, a routine's surface
    pub turn: TurnCtx,
    pub job: JobId,
    pub settings: Arc<Settings>,
    st: Mutex<RunState>,
}

impl Run {
    pub fn new(ctx: Arc<HubCtx>, mut turn: TurnCtx, job: JobId, settings: Arc<Settings>) -> Self {
        turn.job = Some(job);
        Self { ctx, turn, job, settings, st: Mutex::new(RunState::default()) }
    }

    pub fn state(&self) -> MutexGuard<'_, RunState> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `setJob(job, phase, detail)`.
    pub fn set_job(&self, phase: &str, detail: &str) {
        self.ctx.jobs.set(&self.ctx.store, self.job, phase, detail);
    }

    /// `labelOf(modelCtx.getStore())`.
    pub fn label(&self) -> String {
        label_of(self.turn.model.as_ref(), &self.settings)
    }

    /// The links and next steps the tools left, as the reply's fields.
    pub fn link_fields(&self) -> (Option<Value>, Option<Value>) {
        let st = self.state();
        let Some(lt) = &st.link_turn else { return (None, None) };
        ((!lt.links.is_empty()).then(|| Value::Array(lt.links.clone())), lt.suggest.as_ref().filter(|s| !s.is_empty()).map(|s| Value::Array(s.clone())))
    }
}

// The Anthropic server-side web_search gets one shot: if this build of the
// API or model rejects it, the client-side search takes over for the
// process (tools.mjs `serverSearch`).
static SERVER_SEARCH: AtomicBool = AtomicBool::new(true);

pub fn server_search() -> bool {
    SERVER_SEARCH.load(Ordering::SeqCst)
}

pub fn set_server_search(on: bool) {
    SERVER_SEARCH.store(on, Ordering::SeqCst);
}

/// `toolList(thread, job)`: the tools a model call offers.
pub async fn tool_list(run: &Run) -> Vec<Value> {
    let (mcp, deferred) = mcp::surface(run).await;
    let load: Vec<Value> = if deferred.is_empty() { Vec::new() } else { vec![mcp::app_tools_load_def(&deferred)] };
    // a v1 routine's turn gets exactly its surface (5.9)
    if run.turn.surface.is_some() {
        return mcp.into_iter().chain(load).collect();
    }
    let office = office::tools(&run.ctx);
    let bot = run.ctx.store.read(|s| botctx::bot_of(s, &run.turn).is_some());
    let own: &[Value] = if bot { defs::bot_tools() } else { &[] };
    // a routine's own turn doesn't make, change or run routines
    let routine: &[Value] = if run.turn.routine.is_some() { &[] } else { defs::routine_tools() };
    // an agent's own turns message and make agents (agents-tools.mjs)
    let agent: &[Value] = if run.turn.routine.is_some() || run.turn.surface.is_some() || !bot { &[] } else { defs::agent_tools() };
    let mut out = Vec::new();
    if server_search() {
        out.push(json!({"type": "web_search_20250305", "name": "web_search", "max_uses": 3}));
    }
    out.extend(defs::tools().iter().cloned());
    out.extend(routine.iter().cloned());
    out.extend(own.iter().cloned());
    out.extend(agent.iter().cloned());
    out.extend(office);
    out.extend(load);
    out.extend(mcp);
    if !server_search() {
        out.push(defs::client_search().clone());
    }
    out
}

/// `execTool(job, thread, block)`: a tool call's answer; a failure inside a
/// tool is `tool error: …`.
pub async fn exec_tool(run: &Run, block: &Value) -> Out {
    let name = block.get("name").map(js::string_of).unwrap_or_default();
    let input = block.get("input").filter(|i| js::truthy(i)).cloned().unwrap_or_else(|| json!({}));
    match dispatch(run, &name, &input).await {
        Ok(out) => out,
        Err(e) => Out::err(format!("tool error: {}", js::slice_utf16(&e, 500))),
    }
}

fn is_card_name(v: Option<&Value>) -> bool {
    let s = v.filter(|v| !v.is_null()).map(js::string_of).unwrap_or_default();
    s.strip_prefix('R').is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

async fn dispatch(run: &Run, name: &str, input: &Value) -> Result<Out, String> {
    let ctx = &run.ctx;
    match name {
        "generate_app" => return artifact::generate_app(run, input).await,
        "revise_app" => {
            if !is_card_name(input.get("name")) {
                return Ok(Out::err("revise_app needs the artifact name (R<n>)".into()));
            }
            return artifact::generate_app(run, input).await;
        }
        "generate_image" => return artifact::generate_image(run, input).await,
        "react" => return builtin::react(run, input),
        "app_tools_load" => return Ok(mcp::app_tools_load(run, input).await),
        "fetch_url" => return builtin::fetch_url(input).await,
        "shell_run" => return Ok(builtin::shell_run(input).await),
        "query_source" => return Ok(builtin::query_source(input).await),
        "web_search" => return builtin::web_search(input).await,
        "app_icon" => return artifact::app_icon(run, input).await,
        "app_details_update" => return artifact::app_details_update(run, input).await,
        "launch_app" | "close_app" | "promote_app" | "demote_app" | "set_version" | "view_code" => return artifact::artifact_tool(run, name, input).await,
        "mcp_install" | "mcp_uninstall" | "mcp_enable" | "mcp_disable" | "mcp_list" => return mcp::admin(run, name, input).await,
        _ => {}
    }
    if name.starts_with("office_") {
        return office::tool(run, name, input).await;
    }
    // the skill, bot, routine and agent tools: their tracks' handlers
    if let Some(h) = ctx.tools.get(name) {
        let r = h.call(ctx.clone(), run.turn.clone(), input.clone()).await;
        return Ok(Out { content: r.content, is_error: r.is_error.then_some(true) });
    }
    // a v1 plugin's tool, under its own name
    if v1::tool_plugin(v1::view(ctx).as_ref(), name, run.turn.surface.as_ref()).is_some() {
        run.set_job("connecting", "Working in a connected app");
        return Ok(v1::call(run, name, input).await);
    }
    if let Some(r) = mcp::exec(run, name, input).await {
        return r;
    }
    Ok(Out::err(format!("unknown tool {name}")))
}
