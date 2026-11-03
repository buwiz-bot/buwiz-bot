//! The shell's sandbox: `shell_run` and `shell_find_program` run under `sandbox-exec` with a
//! profile that keeps the Bot's shell away from what would let it change its own rules or act
//! as the app: the host token, app.db (with its -wal, -shm and -journal) and the data dir's
//! root. The files folder (`generated/files`) stays open: replies read and write there. The
//! policy lives in app.db, so there is no separate policy file to deny.

use std::path::{Path, PathBuf};

use super::gate::real;

pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Why the shell cannot run when `sandbox-exec` is missing (it fails closed).
pub const NO_SANDBOX: &str = "shell_run can't run: /usr/bin/sandbox-exec is missing, so commands can't be sandboxed on this Mac.";

/// A path as an SBPL string; `None` for one the profile cannot quote safely.
fn quoted(p: &Path) -> Option<String> {
    let s = p.to_str()?;
    (!s.contains(['"', '\\', '\n', '\r'])).then(|| format!("\"{s}\""))
}

/// `s` with every regex metacharacter escaped.
fn regex_escaped(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if "\\.^$|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The files the shell may neither read nor write, under `data`.
pub fn guarded(data: &Path) -> Vec<PathBuf> {
    let db = data.join("generated").join("app.db");
    let mut out = vec![data.join("host-token"), db.clone()];
    for ext in ["-wal", "-shm", "-journal"] {
        out.push(PathBuf::from(format!("{}{ext}", db.display())));
    }
    out
}

/// The profile for a data dir at `data` (as given and resolved: `/tmp` is `/private/tmp`);
/// `None` when a path cannot be written into it safely (the shell then does not run).
pub fn profile(data: &Path) -> Option<String> {
    let mut roots: Vec<PathBuf> = vec![real(data)];
    if data.is_absolute() && !roots.contains(&data.to_path_buf()) {
        roots.push(data.to_path_buf());
    }
    let mut out = String::from("(version 1)(allow default)");
    for root in &roots {
        let r = quoted(root)?;
        let text = root.to_str()?.trim_end_matches('/');
        out.push_str(&format!("(deny file-write* (literal {r}) (regex #\"^{}/[^/]+$\"))", regex_escaped(text)));
        let files: Vec<String> = guarded(root).iter().map(|f| quoted(f).map(|q| format!("(literal {q})"))).collect::<Option<_>>()?;
        out.push_str(&format!("(deny file-read* file-write* {})", files.join(" ")));
    }
    Some(out)
}

/// `/usr/bin/sandbox-exec -p <profile> /bin/zsh -lc <cmd> [args]`, or why it cannot run.
pub fn command(data: &Path, script: &str, args: &[&str]) -> Result<tokio::process::Command, String> {
    if !Path::new(SANDBOX_EXEC).exists() {
        return Err(NO_SANDBOX.into());
    }
    let Some(p) = profile(data) else { return Err("shell_run can't run: the broker's data dir has a path the sandbox can't name.".into()) };
    let mut cmd = tokio::process::Command::new(SANDBOX_EXEC);
    cmd.arg("-p").arg(p).arg("/bin/zsh").arg("-lc").arg(script);
    if !args.is_empty() {
        cmd.arg("zsh").args(args);
    }
    Ok(cmd)
}

/// `whence -ap -- <name>` in the sandbox, then each hit resolved: `[{path, target}]`. It never
/// runs the program. `name` must be a bare command name.
pub async fn find_program(data: &Path, name: &str) -> Result<Vec<(String, String)>, String> {
    let ok = !name.is_empty() && name.len() <= 128 && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-')) && !name.starts_with('-');
    if !ok {
        return Err("shell_find_program needs a bare command name, e.g. claude".into());
    }
    let mut cmd = command(data, "whence -ap -- \"$1\"", &[name])?;
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).kill_on_drop(true);
    let out = tokio::time::timeout(std::time::Duration::from_secs(15), cmd.output()).await.map_err(|_| "shell_find_program timed out".to_owned())?.map_err(|e| e.to_string())?;
    let mut found: Vec<(String, String)> = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines().map(str::trim).filter(|l| l.starts_with('/')) {
        if found.iter().any(|(p, _)| p == line) {
            continue;
        }
        let target = std::fs::canonicalize(line).map(|t| t.display().to_string()).unwrap_or_else(|_| line.to_owned());
        found.push((line.to_owned(), target));
    }
    Ok(found)
}
