//! Local tools (plan R7): the commands this Mac runs for the hub
//! (osascript, pbcopy, open, sips, qlmanage, magick, …), from PATH as Node's
//! `execFile` finds them, so the tests' PATH stubs answer. The hub reaches
//! them through `ctx.local` (`hub::traits::LocalTools`); [`MacTools`] is
//! this Mac's.

use std::process::Stdio;
use std::sync::Arc;

use tokio::io::AsyncWriteExt;

use crate::hub::ctx::HubCtx;
use crate::hub::traits::{LocalCommand, LocalOutput, LocalTools};
use crate::hub::types::BoxFuture;

/// Runs each command as a child process of the broker.
pub struct MacTools;

impl LocalTools for MacTools {
    fn run(&self, cmd: LocalCommand) -> BoxFuture<'static, Result<LocalOutput, String>> {
        Box::pin(async move {
            let mut c = tokio::process::Command::new(&cmd.program);
            c.args(&cmd.args).stdin(if cmd.stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
            let mut child = c.spawn().map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => format!("spawn {} ENOENT", cmd.program),
                _ => format!("spawn {} {e}", cmd.program),
            })?;
            if let (Some(input), Some(mut pipe)) = (cmd.stdin.clone(), child.stdin.take()) {
                let _ = pipe.write_all(&input).await;
                drop(pipe);
            }
            let waited = child.wait_with_output();
            let out = match cmd.timeout {
                Some(t) => match tokio::time::timeout(t, waited).await {
                    Ok(r) => r,
                    Err(_) => return Ok(LocalOutput { code: None, stdout: Vec::new(), stderr: Vec::new() }),
                },
                None => waited.await,
            }
            .map_err(|e| e.to_string())?;
            Ok(LocalOutput { code: out.status.code(), stdout: out.stdout, stderr: out.stderr })
        })
    }
}

/// The hub's local tools: the ones a track installed, else this Mac's.
pub fn tools(ctx: &HubCtx) -> Arc<dyn LocalTools> {
    ctx.local.clone().unwrap_or_else(|| Arc::new(MacTools))
}

/// A command as `execFile` gets it.
pub fn command(program: &str, args: &[&str]) -> LocalCommand {
    LocalCommand { program: program.into(), args: args.iter().map(|a| (*a).to_owned()).collect(), stdin: None, timeout: None }
}

/// Runs it: `Ok(stdout)` on exit 0, else Node's error message
/// (`Command failed: <cmd> <args>\n<stderr>`, or `spawn <cmd> ENOENT`).
pub async fn exec(ctx: &HubCtx, cmd: LocalCommand) -> Result<String, String> {
    let shown = std::iter::once(cmd.program.clone()).chain(cmd.args.iter().cloned()).collect::<Vec<_>>().join(" ");
    let out = tools(ctx).run(cmd).await?;
    if out.code == Some(0) {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("Command failed: {shown}\n{}", String::from_utf8_lossy(&out.stderr)))
    }
}
