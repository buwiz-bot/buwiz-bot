//! The seams between the hub's tracks: each is a trait here so a track can
//! build and test against a stub before the one implementing it lands (as
//! the JS tests stub `runTurn`). The clock is the plugin host's `Clock`
//! (`Clock::manual()` in tests). Tools register by name in a
//! `ToolRegistry`, each track from its own files; the tool loop (track B)
//! owns only the dispatch.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

pub use crate::helper::plugins::Clock;

use super::ctx::HubCtx;
use super::types::{BoxFuture, ToolResult, TurnCtx};

/// A turn to run in a thread, for routines (5.5) and agents (5.6); the
/// tool loop (5.4) implements it. Node's `routineTurn` / `backgroundTurn`.
#[derive(Clone, Debug, Default)]
pub struct TurnRequest {
    pub turn: TurnCtx,
    /// the user turn the thread shows
    pub shown: String,
    /// what the model is sent
    pub prompt: String,
    /// the shown turn's mark (`routine`, `agent`)
    pub mark: Option<Value>,
}

/// Where a turn landed: its index in the thread and its uid.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnDone {
    pub seq: usize,
    pub turn: String,
}

/// A `/chat` or `/retry` job (agent.mjs `generate(job, prompt)`): its job
/// is queued and answered already; `generate` runs it to its end through
/// `ctx.jobs` (`finish`). An `Err` ends the job as Node's crash does
/// ("generator error — <e>").
#[derive(Clone, Debug, Default)]
pub struct ChatTurn {
    /// the thread and the job (`turn.job`); `turn.model` is `None`: `/chat`
    /// runs under the thread's `choiceFor`, `/retry` under none (Node runs
    /// a retry outside `modelCtx`)
    pub turn: TurnCtx,
    /// the user's message
    pub prompt: String,
    /// `job.extra`: `reply` (`{i, text, files?}`), `attachments`,
    /// `v1Attachments`, as the turn carries them
    pub extra: Value,
    /// `/retry`'s card name (`job.retry`)
    pub retry: Option<String>,
}

pub trait TurnRunner: Send + Sync {
    fn run(&self, ctx: Arc<HubCtx>, turn: TurnRequest) -> BoxFuture<'static, Result<TurnDone, String>>;

    /// `/chat` and `/retry`'s turn. Until the tool loop (5.4) implements
    /// it, a runner answers `not ported`.
    fn generate(&self, _ctx: Arc<HubCtx>, _chat: ChatTurn) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async { Err("not ported".to_owned()) })
    }
}

/// The model gateway (5.3): an Anthropic Messages body in, the final
/// message out, whole or as its stream's events.
pub trait ModelApi: Send + Sync {
    fn create(&self, turn: &TurnCtx, body: Value) -> BoxFuture<'static, Result<Value, String>>;
    fn stream(&self, turn: &TurnCtx, body: Value) -> BoxFuture<'static, Result<tokio::sync::mpsc::Receiver<Result<Value, String>>, String>>;
}

/// `nsh build` and `nsh check` (5.7a): `Ok` on a pass, else the output
/// (stdout and stderr; "" when the binary is missing, still a failure).
pub trait Nsh: Send + Sync {
    fn build(&self, entry: &Path, out: &Path) -> BoxFuture<'static, Result<(), String>>;
    fn check(&self, entry: &Path) -> BoxFuture<'static, Result<(), String>>;
}

/// A command this Mac runs for the hub (`zsh -lc`, osascript, pbcopy,
/// open, sips, `secret get`, …): `helper/local.rs` runs it, tests stub it.
#[derive(Clone, Debug, Default)]
pub struct LocalCommand {
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<Vec<u8>>,
    pub timeout: Option<Duration>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalOutput {
    /// the exit code; `None` when it was killed (a timeout, a signal)
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub trait LocalTools: Send + Sync {
    fn run(&self, cmd: LocalCommand) -> BoxFuture<'static, Result<LocalOutput, String>>;
}

/// A tool the model may call: its input in, its result out.
pub trait ToolHandler: Send + Sync {
    fn call(&self, ctx: Arc<HubCtx>, turn: TurnCtx, input: Value) -> BoxFuture<'static, ToolResult>;
}

impl<F, Fut> ToolHandler for F
where
    F: Fn(Arc<HubCtx>, TurnCtx, Value) -> Fut + Send + Sync,
    Fut: Future<Output = ToolResult> + Send + 'static,
{
    fn call(&self, ctx: Arc<HubCtx>, turn: TurnCtx, input: Value) -> BoxFuture<'static, ToolResult> {
        Box::pin(self(ctx, turn, input))
    }
}

/// The tools' handlers by name.
#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn ToolHandler>>,
}

impl ToolRegistry {
    /// Adds a tool. Two tracks claiming one name is a bug: it panics at
    /// startup, where every test sees it.
    pub fn register(&mut self, name: &str, handler: impl ToolHandler + 'static) {
        let previous = self.tools.insert(name.to_owned(), Arc::new(handler));
        assert!(previous.is_none(), "the tool {name} is registered twice");
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        self.tools.get(name).cloned()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(String::as_str)
    }
}
