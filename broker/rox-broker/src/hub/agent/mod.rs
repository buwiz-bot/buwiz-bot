//! The tool loop (plan 5.4; agent.mjs): the hub's `TurnRunner`. `/chat`
//! and `/retry` jobs run `generate` (`generate.rs`): the slash commands
//! (`slash_mock.rs`), the tool loop (`turn.rs`, with the streamed
//! placeholders of `placeholders.rs` and the user's message as
//! `user_content.rs` builds it), the chat reply, the legacy generator.
//! Routines' and agents' turns run `backgroundTurn`.
//!
//! What Node keeps in AsyncLocalStorage is explicit here: the model choice
//! (`modelCtx`) is `TurnCtx::model`, picked by this runner (`/chat` and
//! background turns under `choiceFor(thread)`, `/retry` under none, Node's
//! MODEL-01 bug), and the acting bot is `TurnCtx::acting_bot`.

pub mod files;
pub mod generate;
pub mod placeholders;
pub mod slash_mock;
pub mod turn;
pub mod user_content;

use std::sync::Arc;

use serde_json::{json, Value};

use super::agents::run::{AgentRequest, AgentRequestTurn, RequestDone};
use super::ctx::HubCtx;
use super::model::choice::{choice_for_db, retry_choice, BotModel};
use super::model::Settings;
use super::store::{Thread, Turn};
use super::tools::Run;
use super::traits::{ChatTurn, TurnDone, TurnRequest, TurnRunner};
use super::types::{BoxFuture, ModelChoice, TurnCtx};
use super::Registry;
use crate::office::Office;

/// The hub's turn runner.
pub struct Agent {
    settings: Arc<Settings>,
}

impl Agent {
    pub fn new(settings: Arc<Settings>) -> Self {
        Self { settings }
    }
}

/// `choiceFor(thread)` with the bot that answers there (the acting bot
/// first, as `threadBot` reads it).
pub fn choice_of(ctx: &HubCtx, thread: &str, acting: Option<&str>) -> Option<ModelChoice> {
    let bot = ctx.store.read(|s| {
        let b = acting.and_then(|id| s.bot(id)).or_else(|| s.thread_bot(thread));
        b.map(|b| BotModel { name: b.name.clone(), model: b.model.clone(), effort: b.effort.clone(), fast: b.fast })
    });
    choice_for_db(&ctx.db, bot.as_ref())
}

/// The job's thread: its own, else the active one (`store.list.find(…) ||
/// activeThread()`).
fn thread_of(ctx: &HubCtx, id: &str) -> String {
    ctx.store.read(|s| s.list.iter().find(|t| t.id == id).map(|t| t.id.clone()).unwrap_or_else(|| if s.list.is_empty() { id.to_owned() } else { s.active_thread().id.clone() }))
}

impl TurnRunner for Agent {
    /// `backgroundTurn`: a routine's or an agent's turn. `req.mark` is its
    /// `extra`, spread onto the shown turn as it is (see [`TurnRequest`]).
    fn run(&self, ctx: Arc<HubCtx>, req: TurnRequest) -> BoxFuture<'static, Result<TurnDone, String>> {
        let settings = self.settings.clone();
        Box::pin(async move {
            let mut turn = req.turn;
            let job = match turn.job {
                Some(j) => j,
                None => ctx.jobs.create(&turn.thread, Value::Null, None, false),
            };
            if turn.model.is_none() {
                turn.model = choice_of(&ctx, &turn.thread, turn.acting_bot.as_deref());
            }
            let extra = req.mark.unwrap_or_else(|| json!({}));
            let run = Run::new(ctx, turn, job, settings);
            generate::background_turn(&run, &req.shown, &req.prompt, &extra).await
        })
    }

    /// `/chat`'s and `/retry`'s job.
    fn generate(&self, ctx: Arc<HubCtx>, chat: ChatTurn) -> BoxFuture<'static, Result<(), String>> {
        let settings = self.settings.clone();
        Box::pin(async move {
            let mut turn = chat.turn;
            let job = turn.job.ok_or("no job")?;
            turn.thread = thread_of(&ctx, &turn.thread);
            // BUG: /retry runs outside the thread's model choice (MODEL-01)
            turn.model = if chat.retry.is_some() { retry_choice() } else { choice_of(&ctx, &turn.thread, turn.acting_bot.as_deref()) };
            let run = Run::new(ctx, turn, job, settings);
            generate::generate(&run, &chat.prompt, &chat.extra, chat.retry.as_deref()).await
        })
    }
}

/// An agent's request (agents-wire.mjs `runRequest`): the recipient's turn
/// on a stand-in thread (the conversation from its side, `history`), whose
/// writes are its own and never shown; `agentTurn` with the tools, else
/// `chatReply`. → its answer and the cards it made (`job.linkTurn.links`).
pub async fn request_turn(ctx: Arc<HubCtx>, settings: Arc<Settings>, req: AgentRequest) -> Result<RequestDone, String> {
    let (id, uid, title) = req.thread;
    // allocJobId(): a job of its own, never on the jobs table's thread
    let key = format!("stand-in:{id}:{}", super::store::uuidv7());
    let job = ctx.jobs.create(&key, Value::Null, None, false);
    let turns = req
        .history
        .iter()
        .map(|h| Turn { role: h.get("role").map(crate::js::string_of).unwrap_or_default(), text: h.get("text").filter(|t| !t.is_null()).map(crate::js::string_of), ..Turn::default() })
        .collect();
    ctx.store.touch(|s| s.stand_ins.insert(key.clone(), Thread { id, uid, title, turns, ..Thread::default() }));
    let run = Run::new(ctx.clone(), TurnCtx { thread: key.clone(), ..req.turn }, job, settings);
    let answer = generate::request_reply(&run, &req.prompt).await;
    let links = run.state().link_turn.as_ref().map(|l| l.links.clone()).unwrap_or_default();
    ctx.store.touch(|s| s.stand_ins.remove(&key));
    ctx.jobs.with(job, |j| j.done = true);
    Ok(RequestDone { answer: answer?, links })
}

/// Track B's 5.4: the runner (in place of any other) and an agent's
/// request turn (5.6's seam). The office it calls is installed by
/// `office::install` (6.1, in process; it was D31's Node child).
pub fn install(_routes: &mut Registry, ctx: &mut HubCtx) {
    let settings = Arc::new(Settings::global().clone());
    ctx.turns = Some(Arc::new(Agent::new(settings.clone())));
    ctx.ext.insert(AgentRequestTurn(Arc::new(move |ctx, req| Box::pin(request_turn(ctx, settings.clone(), req)))));
}

/// The hub's office, for a test or a caller outside the hub.
pub fn office_of(ctx: &HubCtx) -> Option<Arc<Office>> {
    ctx.ext.get::<Arc<Office>>().cloned()
}
