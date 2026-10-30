//! `nsh` as a shell-out (`runNsh`, `checkCandidate`): `NSH`, else `nsh` on
//! PATH; 90 s, then SIGTERM. A pass is `Ok`; anything else is `Err` with
//! stdout then stderr as read so far, "" when the binary is missing (still a
//! failure: the card then says "unknown").

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::AsyncReadExt;

use crate::hub::traits::Nsh;
use crate::hub::types::BoxFuture;

/// Node's execFile timeout for nsh.
pub const TIMEOUT: Duration = Duration::from_secs(90);

/// The nsh binary as Node runs it.
#[derive(Clone, Debug)]
pub struct NshCli {
    pub bin: PathBuf,
    pub timeout: Duration,
    /// more environment for nsh (tests steer the stub with it)
    pub env: Vec<(String, String)>,
}

impl NshCli {
    /// config.mjs's NSH: `$NSH`, else `nsh`.
    pub fn from_env() -> Self {
        let bin = std::env::var("NSH").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "nsh".to_owned());
        Self { bin: PathBuf::from(bin), timeout: TIMEOUT, env: Vec::new() }
    }

    /// `runNsh(args)`: `None` on a pass, else the output.
    pub async fn run(&self, args: &[&std::ffi::OsStr]) -> Option<String> {
        let mut cmd = tokio::process::Command::new(&self.bin);
        cmd.args(args).envs(self.env.iter().map(|(k, v)| (k, v))).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let Ok(mut child) = cmd.spawn() else { return Some(String::new()) };
        let (out, err) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())));
        let drain = |pipe: Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>>, buf: Arc<Mutex<Vec<u8>>>| {
            tokio::spawn(async move {
                let Some(mut pipe) = pipe else { return };
                let mut chunk = [0u8; 8192];
                while let Ok(n) = pipe.read(&mut chunk).await {
                    if n == 0 {
                        break;
                    }
                    buf.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&chunk[..n]);
                }
            })
        };
        let readers = [
            drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), out.clone()),
            drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), err.clone()),
        ];
        let passed = match tokio::time::timeout(self.timeout, child.wait()).await {
            Ok(Ok(status)) => status.success(),
            Ok(Err(_)) => false,
            Err(_) => {
                // execFile's timeout: SIGTERM, and whatever it printed
                if let Some(pid) = child.id() {
                    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                }
                let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
                false
            }
        };
        for r in readers {
            let _ = tokio::time::timeout(Duration::from_secs(2), r).await;
        }
        if passed {
            return None;
        }
        let text = |b: &Arc<Mutex<Vec<u8>>>| String::from_utf8_lossy(&b.lock().unwrap_or_else(|e| e.into_inner())).into_owned();
        Some(text(&out) + &text(&err))
    }
}

impl Nsh for NshCli {
    fn build(&self, entry: &Path, out: &Path) -> BoxFuture<'static, Result<(), String>> {
        let (me, entry, out) = (self.clone(), entry.to_owned(), out.to_owned());
        Box::pin(async move { me.run(&[std::ffi::OsStr::new("build"), entry.as_os_str(), std::ffi::OsStr::new("--out"), out.as_os_str()]).await.map_or(Ok(()), Err) })
    }

    fn check(&self, entry: &Path) -> BoxFuture<'static, Result<(), String>> {
        let (me, entry) = (self.clone(), entry.to_owned());
        Box::pin(async move { me.run(&[std::ffi::OsStr::new("check"), entry.as_os_str()]).await.map_or(Ok(()), Err) })
    }
}

/// `checkCandidate(name)`: `.stage/<name>.tsx` (written by the caller)
/// compiled through a mounting entry, then type-checked; the pair removed
/// once the verdict is in. `Err` is nsh's text ("" for a missing binary).
pub async fn check_candidate(nsh: &dyn Nsh, stage: &Path, name: &str) -> Result<(), String> {
    let entry = stage.join(format!("check-{name}.tsx"));
    let module = stage.join(format!("{name}.tsx"));
    let _ = std::fs::write(&entry, format!("import {{ mount }} from \"nsh\";\nimport R from \"./{name}\";\nmount(R);\n"));
    let verdict = match nsh.build(&entry, &stage.join(format!("out-{name}"))).await {
        Ok(()) => nsh.check(&entry).await,
        Err(e) => Err(e),
    };
    // rmSync(entry); rmSync(mod) in one try: a missing entry keeps the module
    if std::fs::remove_file(&entry).is_ok() {
        let _ = std::fs::remove_file(&module);
    }
    verdict
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The broker tests' nsh stub (server/test/fixtures/bin/nsh), logging
    /// to `log`, in mode `mode`.
    pub fn stub(log: &Path, mode: &str) -> NshCli {
        let bin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../server/test/fixtures/bin/nsh");
        NshCli { bin, timeout: TIMEOUT, env: vec![("ROX_STUB_LOG".into(), log.display().to_string()), ("NSH_STUB".into(), mode.into())] }
    }

    /// The stub's recorded argv, oldest first.
    pub fn calls(log: &Path) -> Vec<Vec<String>> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|c| c["bin"] == "nsh")
            .map(|c| c["argv"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_owned()).collect())
            .collect()
    }

    pub fn temp(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rox-pipe-{label}-{}-{:?}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[tokio::test]
    async fn pipe_01_run_nsh_pass_failure_text_and_missing() {
        let d = temp("nsh");
        let log = d.join("stubs.log");
        fn os(v: &[&'static str]) -> Vec<&'static std::ffi::OsStr> {
            v.iter().map(|s| std::ffi::OsStr::new(*s)).collect()
        }
        assert_eq!(stub(&log, "pass").run(&os(&["check", "x.tsx"])).await, None);
        assert_eq!(stub(&log, "fail").run(&os(&["build", "y.tsx", "--out", "o"])).await.as_deref(), Some("error TS2304: Cannot find name 'nope'.\n"));
        let mut custom = stub(&log, "fail");
        custom.env.push(("NSH_STUB_TEXT".into(), "line one\nline two".into()));
        assert_eq!(custom.run(&os(&["check", "z.tsx"])).await.as_deref(), Some("line one\nline two\n"));
        assert_eq!(stub(&log, "missing").run(&os(&["check", "x.tsx"])).await.as_deref(), Some(""));
        let absent = NshCli { bin: d.join("no-such-nsh"), timeout: TIMEOUT, env: vec![] };
        assert_eq!(absent.run(&os(&["check", "x.tsx"])).await.as_deref(), Some(""));
        assert_eq!(calls(&log), vec![s(&["check", "x.tsx"]), s(&["build", "y.tsx", "--out", "o"]), s(&["check", "z.tsx"]), s(&["check", "x.tsx"])]);
    }

    #[tokio::test]
    async fn pipe_01_a_hung_nsh_times_out_with_what_it_printed() {
        // a shell as "nsh", told what to run: it prints, then hangs
        let nsh = NshCli { bin: PathBuf::from("/bin/sh"), timeout: Duration::from_millis(1500), env: vec![] };
        let t = std::time::Instant::now();
        let out = nsh.run(&[std::ffi::OsStr::new("-c"), std::ffi::OsStr::new("echo started; exec sleep 30")]).await;
        assert_eq!(out.as_deref(), Some("started\n"));
        assert!(t.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn pipe_03_to_05_check_candidate() {
        // a pass: build then check, the stage pair removed, the output kept
        let d = temp("check");
        let (log, stage) = (d.join("stubs.log"), d.join(".stage"));
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("R5.tsx"), "x").unwrap();
        assert_eq!(check_candidate(&stub(&log, "pass"), &stage, "R5").await, Ok(()));
        let entry = stage.join("check-R5.tsx").display().to_string();
        assert_eq!(calls(&log), vec![s(&["build", &entry, "--out", &stage.join("out-R5").display().to_string()]), s(&["check", &entry])]);
        assert!(!stage.join("check-R5.tsx").exists() && !stage.join("R5.tsx").exists());
        assert_eq!(std::fs::read_to_string(stage.join("out-R5/bundle.js")).unwrap(), "// nsh stub bundle of check-R5.tsx\nexport default null;\n");

        // a failing build (the candidate marked): its text, no check
        let d = temp("check-fail");
        let (log, stage) = (d.join("stubs.log"), d.join(".stage"));
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("R7.tsx"), "x // nsh-stub:fail\n").unwrap();
        assert_eq!(check_candidate(&stub(&log, "pass"), &stage, "R7").await, Err("error TS2304: Cannot find name 'nope'.\n".into()));
        assert_eq!(calls(&log).iter().map(|c| c[0].as_str()).collect::<Vec<_>>(), ["build"]);
        assert!(!stage.join("check-R7.tsx").exists() && !stage.join("R7.tsx").exists());

        // nsh missing: "", still a failure, no check
        let d = temp("check-missing");
        let (log, stage) = (d.join("stubs.log"), d.join(".stage"));
        std::fs::create_dir_all(&stage).unwrap();
        assert_eq!(check_candidate(&stub(&log, "missing"), &stage, "R2").await, Err(String::new()));
        assert_eq!(calls(&log).len(), 1);
    }
}
