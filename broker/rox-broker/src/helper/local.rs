//! The hub's local tools on this Mac (rule R7): a command run as Node's
//! `execFile` runs it, found on the broker's PATH (so the test stubs stand
//! in), its environment the broker's plus the command's own.

use std::process::Stdio;

use tokio::io::AsyncWriteExt;

use crate::hub::traits::{LocalCommand, LocalOutput, LocalTools};
use crate::hub::types::BoxFuture;

/// Runs each command as a child of the broker.
pub struct MacTools;

/// A spawn that failed, as libuv names it (`ENOENT`, `EACCES`, …).
fn spawn_code(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "ENOENT".to_owned(),
        std::io::ErrorKind::PermissionDenied => "EACCES".to_owned(),
        _ => e.to_string(),
    }
}

impl LocalTools for MacTools {
    /// `Err` with the spawn's error code when it could not start; a run
    /// past its timeout is killed and comes back with no exit code.
    fn run(&self, cmd: LocalCommand) -> BoxFuture<'static, Result<LocalOutput, String>> {
        Box::pin(async move {
            let mut c = tokio::process::Command::new(&cmd.program);
            c.args(&cmd.args)
                .envs(cmd.env.iter().map(|(k, v)| (k, v)))
                .stdin(if cmd.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            let mut child = c.spawn().map_err(|e| spawn_code(&e))?;
            if let (Some(input), Some(mut pipe)) = (cmd.stdin, child.stdin.take()) {
                let _ = pipe.write_all(&input).await;
            }
            let out = child.wait_with_output();
            let out = match cmd.timeout {
                Some(t) => match tokio::time::timeout(t, out).await {
                    Ok(out) => out,
                    Err(_) => return Ok(LocalOutput { code: None, stdout: Vec::new(), stderr: Vec::new() }),
                },
                None => out.await,
            }
            .map_err(|e| e.to_string())?;
            Ok(LocalOutput { code: out.status.code(), stdout: out.stdout, stderr: out.stderr })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_runs_with_env_stdin_and_a_timeout() {
        let out = MacTools
            .run(LocalCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "printf '%s:' \"$ROX_LOCAL_T\"; cat; exit 3".into()],
                env: vec![("ROX_LOCAL_T".into(), "hi".into())],
                stdin: Some(b"in".to_vec()),
                timeout: None,
            })
            .await
            .unwrap();
        assert_eq!((out.code, out.stdout.as_slice()), (Some(3), b"hi:in".as_slice()));
        let slow = MacTools.run(LocalCommand { program: "/bin/sleep".into(), args: vec!["5".into()], timeout: Some(std::time::Duration::from_millis(50)), ..Default::default() }).await.unwrap();
        assert_eq!(slow.code, None);
        assert_eq!(MacTools.run(LocalCommand { program: "/no/such/tool".into(), ..Default::default() }).await, Err("ENOENT".to_owned()));
    }
}
