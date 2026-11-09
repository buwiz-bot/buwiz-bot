//! The connection to an SSH host: one `ssh` master per computer, and each command a client of it.
//!
//! `/usr/bin/ssh`, and nothing of this login's SSH setup: no config, no agent, no known hosts. The
//! far end proves itself against the one key a person pinned, and is reached with the one key they
//! gave. That key is on disk only until the master has logged in. The master's socket lets anything
//! on it act as that login, so it's in the computer's folder, which the shell's sandbox closes, with
//! unix sockets shut besides.
//!
//! What a far end's `ssh` prints isn't passed on: a server's banner could say anything, so it's
//! matched to a sentence of ours. A remote process isn't stopped by a timeout here: the connection
//! to it is. What it does after that is that machine's.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

use crate::{Problem, Result, Secret};

pub const SSH: &str = "/usr/bin/ssh";
pub const KEYSCAN: &str = "/usr/bin/ssh-keyscan";

/// What a test of a connection's folder is called, under `computers`.
pub const PROBE_PREFIX: &str = "probe-";
const FOLDER: &str = "ssh";
const KEY_FILE: &str = "key";
const KNOWN_HOSTS: &str = "known_hosts";
const SOCKET: &str = "c";
/// The name the pinned key goes by, so it's that key whatever the host is called.
const ALIAS: &str = "computer";
const LOGIN_PATIENCE: Duration = Duration::from_secs(25);
const SCAN_PATIENCE: Duration = Duration::from_secs(10);
const LOGIN_POLL: Duration = Duration::from_millis(50);
const CLOSE_PATIENCE: Duration = Duration::from_secs(3);
/// How much of `ssh`'s own stderr is kept, to match a sentence against.
const MAX_ERROR: usize = 8 * 1024;
const MAX_SCAN: usize = 64 * 1024;

const SIGNED_KEY_TYPES: [&str; 5] = ["ssh-ed25519", "ecdsa-sha2-nistp256", "ecdsa-sha2-nistp384", "ecdsa-sha2-nistp521", "ssh-rsa"];
const WRONG_HOST_KEY: &str = "This machine's SSH key isn't the one you trusted, so nothing was sent. If it changed on purpose, remove this computer and add it again.";
const NOT_ACCEPTED: &str = "That machine didn't accept this key.";
pub(crate) const UNREACHED: &str = "That machine can't be reached.";
const WENT_DOWN: &str = "The connection to this computer dropped. Click it to connect again.";

/// The far end of a connection, each part checked already.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct At {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
}

/// Why a master didn't come up.
#[derive(Debug)]
pub enum Failed {
    /// a forward's port went to someone else since it was picked
    PortTaken,
    Said(Problem),
}

/// What a command left.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ran {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
    pub timed_out: bool,
}

/// `pin` is one key, of a type this takes: it goes in a known_hosts file, so nothing else may.
pub fn check_pin(pin: &str) -> Result<()> {
    let bad = || Problem::Remote("That SSH key isn't one line of type and key.".to_owned());
    let (kind, key) = pin.split_once(' ').ok_or_else(bad)?;
    let sound = SIGNED_KEY_TYPES.contains(&kind) && !key.is_empty() && base64::engine::general_purpose::STANDARD.decode(key).is_ok();
    sound.then_some(()).ok_or_else(bad)
}

/// The fingerprint `ssh-keygen -l` prints for `pin`.
pub fn fingerprint(pin: &str) -> Result<String> {
    check_pin(pin)?;
    let key = pin.split_once(' ').map_or("", |(_, k)| k);
    let raw = base64::engine::general_purpose::STANDARD.decode(key).map_err(|_| Problem::Remote("That SSH key isn't base64.".to_owned()))?;
    Ok(format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(raw))))
}

/// Asks `host` for its key, trusting nothing: the answer is for a person to check.
pub async fn scan(program: &Path, host: &str, port: Option<u16>) -> Result<(String, String)> {
    let port = port.unwrap_or(22).to_string();
    let child = Command::new(program)
        .args(["-T", "5", "-t", "ed25519,ecdsa,rsa", "-p", &port, "--", host])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(SCAN_PATIENCE, child).await.map_err(|_| Problem::Remote(UNREACHED.to_owned()))?.map_err(|_| Problem::ProviderMissing("ssh-keyscan".to_owned()))?;
    let text = String::from_utf8_lossy(&out.stdout[..out.stdout.len().min(MAX_SCAN)]).into_owned();
    let pin = pick_key(&text).ok_or_else(|| Problem::Remote(UNREACHED.to_owned()))?;
    Ok((fingerprint(&pin)?, pin))
}

/// The best key of `ssh-keyscan`'s lines, as `<type> <key>`.
fn pick_key(lines: &str) -> Option<String> {
    let keys: Vec<String> = lines.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split_whitespace().skip(1).take(2).collect::<Vec<_>>().join(" ").into()).filter(|k: &String| check_pin(k).is_ok()).collect();
    SIGNED_KEY_TYPES.iter().find_map(|kind| keys.iter().find(|k| k.starts_with(&format!("{kind} "))).cloned())
}

/// What every `ssh` here runs with. No line of it comes from this login's setup, or from the far end.
fn options(at: &At) -> Vec<String> {
    let mut out: Vec<String> = ["-F", "/dev/null", "-T"].map(str::to_owned).to_vec();
    for option in [
        "BatchMode=yes",
        "StrictHostKeyChecking=yes",
        "UpdateHostKeys=no",
        "CheckHostIP=no",
        "VerifyHostKeyDNS=no",
        "GlobalKnownHostsFile=/dev/null",
        "IdentitiesOnly=yes",
        "IdentityAgent=none",
        "AddKeysToAgent=no",
        "PubkeyAuthentication=yes",
        "PasswordAuthentication=no",
        "KbdInteractiveAuthentication=no",
        "GSSAPIAuthentication=no",
        "HostbasedAuthentication=no",
        "ForwardAgent=no",
        "ForwardX11=no",
        "PermitLocalCommand=no",
        "ProxyCommand=none",
        "ProxyJump=none",
        "Tunnel=no",
        "RequestTTY=no",
        "EscapeChar=none",
        "ConnectTimeout=10",
        "ServerAliveInterval=15",
        "ServerAliveCountMax=3",
        "ExitOnForwardFailure=yes",
        "LogLevel=ERROR",
        "ControlPersist=no",
    ] {
        out.extend(["-o".to_owned(), option.to_owned()]);
    }
    out.extend(["-o".to_owned(), format!("HostKeyAlias={ALIAS}")]);
    out.extend(["-o".to_owned(), format!("UserKnownHostsFile={KNOWN_HOSTS}")]);
    out.extend(["-o".to_owned(), format!("ControlPath={SOCKET}")]);
    if let Some(port) = at.port {
        out.extend(["-p".to_owned(), port.to_string()]);
    }
    if let Some(user) = &at.user {
        out.extend(["-l".to_owned(), user.clone()]);
    }
    out
}

fn ssh(program: &Path, dir: &Path, args: &[String]) -> Command {
    let mut command = Command::new(program);
    command.args(args).current_dir(dir).env_clear().kill_on_drop(true);
    command
}

/// `value` as one word to a POSIX shell.
pub(crate) fn quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// The line the far end's login shell gets: `/bin/sh` from there on, whatever that shell is.
fn remote_line(script: &str, args: &[&str]) -> String {
    let words: Vec<String> = args.iter().map(|a| quoted(a)).collect();
    format!("exec /bin/sh -c {} sh {}", quoted(script), words.join(" ")).trim_end().to_owned()
}

/// `ssh`'s own stderr as one of our sentences.
fn sentence(stderr: &str) -> Problem {
    let low = stderr.to_ascii_lowercase();
    let says = |words: &[&str]| words.iter().any(|w| low.contains(w));
    Problem::Remote(
        if says(&["host key verification failed", "identification has changed", "host key for"]) {
            WRONG_HOST_KEY
        } else if says(&["permission denied", "too many authentication failures", "invalid format", "load key"]) {
            NOT_ACCEPTED
        } else {
            UNREACHED
        }
        .to_owned(),
    )
}

fn forward_lost(stderr: &str) -> bool {
    let low = stderr.to_ascii_lowercase();
    low.contains("cannot listen") || low.contains("address already in use") || low.contains("forwarding failed")
}

fn io_failed(e: &std::io::Error) -> Problem {
    Problem::Failed(format!("its connection's folder: {}", e.kind()))
}

/// Takes away what a broker that stopped outright left under `computers`, a computers folder: a key
/// staged for a login, and a test's whole folder. A running master needs neither.
pub fn clear_staged(computers: &Path) {
    let Ok(entries) = std::fs::read_dir(computers) else { return };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(PROBE_PREFIX) {
            let _ = std::fs::remove_dir_all(entry.path());
        } else {
            let _ = std::fs::remove_file(entry.path().join(FOLDER).join(KEY_FILE));
        }
    }
}

/// The key, on disk until this goes: `ssh` wants a file, and only the login reads it.
struct StagedKey(PathBuf);

impl StagedKey {
    fn new(dir: &Path, key: &Secret) -> Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let path = dir.join(KEY_FILE);
        let _ = std::fs::remove_file(&path);
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).map_err(|e| io_failed(&e))?;
        std::io::Write::write_all(&mut file, key.expose().as_bytes()).map_err(|e| io_failed(&e))?;
        // `ssh` reads a key to its end, and a key's last line must have one
        std::io::Write::write_all(&mut file, b"\n").map_err(|e| io_failed(&e))?;
        Ok(Self(path))
    }
}

impl Drop for StagedKey {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Keeps reading `from`, into `into` up to `cap` and past that nowhere, and notes it went over.
async fn gather(mut from: impl AsyncRead + Unpin, into: Arc<Mutex<Vec<u8>>>, cap: usize, over: Arc<AtomicBool>) {
    let mut chunk = [0u8; 8192];
    while let Ok(n) = from.read(&mut chunk).await {
        if n == 0 {
            return;
        }
        let mut kept = into.lock().unwrap_or_else(|e| e.into_inner());
        let room = cap.saturating_sub(kept.len()).min(n);
        kept.extend_from_slice(&chunk[..room]);
        if room < n {
            over.store(true, Ordering::Relaxed);
        }
    }
}

/// An open connection. Dropped, its `ssh` goes with it.
pub struct Master {
    program: PathBuf,
    dir: PathBuf,
    at: At,
    child: Mutex<Option<Child>>,
}

impl Master {
    /// Logs in and starts the forwards, each `(port here, port on that machine's loopback)`.
    pub async fn open(program: &Path, computer: &Path, at: &At, pin: &str, key: Option<&Secret>, forwards: &[(u16, u16)]) -> std::result::Result<Self, Failed> {
        let said = Failed::Said;
        check_pin(pin).map_err(said)?;
        let key = key.ok_or_else(|| Failed::Said(Problem::Remote("This computer has no SSH key: remove it and add it again.".to_owned())))?;
        let dir = computer.join(FOLDER);
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::create_dir_all(&dir).map_err(|e| said(io_failed(&e)))?;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| said(io_failed(&e)))?;
        }
        std::fs::write(dir.join(KNOWN_HOSTS), format!("{ALIAS} {pin}\n")).map_err(|e| said(io_failed(&e)))?;
        Self::end_old_master(program, &dir, at).await;
        let staged = StagedKey::new(&dir, key).map_err(said)?;
        let mut args = options(at);
        args.extend(["-M", "-N", "-o", "ControlMaster=yes", "-i", KEY_FILE].map(str::to_owned));
        for (here, there) in forwards {
            args.extend(["-L".to_owned(), format!("127.0.0.1:{here}:127.0.0.1:{there}")]);
        }
        args.extend(["--".to_owned(), at.host.clone()]);
        let mut child = ssh(program, &dir, &args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|_| said(Problem::ProviderMissing("ssh".to_owned())))?;
        let stderr = Arc::new(Mutex::new(Vec::new()));
        if let Some(pipe) = child.stderr.take() {
            tokio::spawn(gather(pipe, stderr.clone(), MAX_ERROR, Arc::default()));
        }
        let mut master = Self { program: program.to_owned(), dir, at: at.clone(), child: Mutex::new(Some(child)) };
        let began = Instant::now();
        loop {
            if !master.alive() {
                // what it said is on its way through the pipe
                tokio::time::sleep(LOGIN_POLL).await;
                let said = String::from_utf8_lossy(&stderr.lock().unwrap_or_else(|e| e.into_inner())).into_owned();
                return Err(if forward_lost(&said) { Failed::PortTaken } else { Failed::Said(sentence(&said)) });
            }
            if master.logged_in().await {
                drop(staged);
                return Ok(master);
            }
            if began.elapsed() >= LOGIN_PATIENCE {
                master.child = Mutex::new(None);
                return Err(Failed::Said(Problem::Remote(UNREACHED.to_owned())));
            }
            tokio::time::sleep(LOGIN_POLL).await;
        }
    }

    /// A broker that went away leaves its master running, on this socket. Closing it is local.
    async fn end_old_master(program: &Path, dir: &Path, at: &At) {
        if dir.join(SOCKET).exists() {
            let mut args = options(at);
            args.extend(["-O", "exit", "--", &at.host].map(str::to_owned));
            let exit = ssh(program, dir, &args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
            let _ = tokio::time::timeout(CLOSE_PATIENCE, exit).await;
            let _ = std::fs::remove_file(dir.join(SOCKET));
        }
    }

    async fn logged_in(&self) -> bool {
        let mut args = options(&self.at);
        args.extend(["-O", "check", "--", &self.at.host].map(str::to_owned));
        if !self.dir.join(SOCKET).exists() {
            return false;
        }
        ssh(&self.program, &self.dir, &args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().await.is_ok_and(|s| s.success())
    }

    /// Whether its `ssh` is still running. A far end gone quiet ends it, by `ServerAliveCountMax`.
    pub fn alive(&self) -> bool {
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    }

    /// Ends the connection. Nothing is sent to the far end but the hang-up.
    pub async fn close(&self) {
        let child = self.child.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(mut child) = child {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(CLOSE_PATIENCE, child.wait()).await;
        }
        let _ = std::fs::remove_file(self.dir.join(SOCKET));
    }

    /// `script` under the far end's `/bin/sh`, its `$1…` being `args`, with `stdin` fed to it.
    /// What comes back is capped at `cap` a stream, and at `patience` the connection to it is cut.
    pub async fn run(&self, script: &str, args: &[&str], stdin: Option<&[u8]>, patience: Duration, cap: usize) -> Result<Ran> {
        let mut line = options(&self.at);
        line.extend(["-o", "ControlMaster=no", "--", &self.at.host].map(str::to_owned));
        line.push(remote_line(script, args));
        let feeds = if stdin.is_some() { Stdio::piped() } else { Stdio::null() };
        let mut child = ssh(&self.program, &self.dir, &line).stdin(feeds).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|_| Problem::ProviderMissing("ssh".to_owned()))?;
        let (stdout, stderr, over) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)));
        let readers = (child.stdout.take().map(|p| tokio::spawn(gather(p, stdout.clone(), cap, over.clone()))), child.stderr.take().map(|p| tokio::spawn(gather(p, stderr.clone(), cap, over.clone()))));
        let feed = child.stdin.take().zip(stdin.map(<[u8]>::to_vec)).map(|(mut pipe, bytes)| {
            tokio::spawn(async move {
                let _ = pipe.write_all(&bytes).await;
            })
        });
        let done = tokio::time::timeout(patience + Duration::from_secs(1), child.wait()).await;
        let timed_out = done.is_err();
        if timed_out {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        if let Some(feed) = feed {
            feed.abort();
        }
        for reader in [readers.0, readers.1].into_iter().flatten() {
            let _ = tokio::time::timeout(CLOSE_PATIENCE, reader).await;
        }
        let code = done.ok().and_then(|d| d.ok()).and_then(|s| s.code());
        let take = |from: &Mutex<Vec<u8>>| std::mem::take(&mut *from.lock().unwrap_or_else(|e| e.into_inner()));
        // 255 is `ssh`'s own failure, unless the master is fine and the command said it
        if code == Some(255) && !self.alive() {
            return Err(Problem::Remote(WENT_DOWN.to_owned()));
        }
        Ok(Ran { code, stdout: take(&stdout), stderr: take(&stderr), truncated: over.load(Ordering::Relaxed), timed_out })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA==";

    #[test]
    fn what_a_crash_left_goes_and_nothing_else_does() {
        let root = std::env::temp_dir().join(format!("buwiz-staged-{}-{}", std::process::id(), crate::random_hex(4).unwrap()));
        let (kept, probe) = (root.join("cmp_1"), root.join("probe-ab"));
        std::fs::create_dir_all(kept.join(FOLDER)).unwrap();
        std::fs::create_dir_all(probe.join(FOLDER)).unwrap();
        for file in [kept.join(FOLDER).join(KEY_FILE), kept.join(FOLDER).join(KNOWN_HOSTS), kept.join("machine.json"), probe.join(FOLDER).join(KEY_FILE)] {
            std::fs::write(file, "x").unwrap();
        }
        clear_staged(&root);
        assert!(!kept.join(FOLDER).join(KEY_FILE).exists() && !probe.exists());
        assert!(kept.join("machine.json").exists() && kept.join(FOLDER).join(KNOWN_HOSTS).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pin_is_one_key_and_nothing_else() {
        assert!(check_pin(KEY).is_ok());
        for bad in ["", "ssh-ed25519", "ssh-dss AAAA", "@cert-authority * ssh-ed25519 AAAA", "ssh-ed25519 AAAA\nother ssh-ed25519 AAAA", "ssh-ed25519 not*base64"] {
            assert!(check_pin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_scan_gives_the_strongest_key_and_ignores_comments_and_junk() {
        let lines = "# host:22 SSH-2.0-OpenSSH\nhost ssh-rsa AAAA\nhost ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA==\nhost junk\n";
        assert_eq!(pick_key(lines).as_deref(), Some(KEY));
        assert_eq!(pick_key("# nothing\n"), None);
    }

    #[test]
    fn a_fingerprint_is_what_ssh_keygen_prints() {
        // `ssh-keygen -lf` on this key
        let raw = base64::engine::general_purpose::STANDARD.decode("AAAAC3NzaC1lZDI1NTE5AAAAIA==").unwrap();
        let want = format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(raw)));
        assert_eq!(fingerprint(KEY).unwrap(), want);
        assert!(!want.ends_with('='));
    }

    #[test]
    fn nothing_of_this_logins_ssh_setup_is_used() {
        let at = At { host: "h".into(), user: Some("u".into()), port: Some(2222) };
        let args = options(&at).join(" ");
        for word in ["-F /dev/null", "StrictHostKeyChecking=yes", "IdentityAgent=none", "ProxyCommand=none", "ForwardAgent=no", "PermitLocalCommand=no", "BatchMode=yes", "GlobalKnownHostsFile=/dev/null", "-p 2222", "-l u"] {
            assert!(args.contains(word), "{word}: {args}");
        }
        assert!(!args.contains("accept-new") && !args.contains("StrictHostKeyChecking=no"));
    }

    #[test]
    fn a_command_is_one_word_however_its_quoted() {
        assert_eq!(quoted("it's"), r"'it'\''s'");
        assert_eq!(remote_line("echo \"$1\"", &["a b'c"]), r#"exec /bin/sh -c 'echo "$1"' sh 'a b'\''c'"#);
    }

    #[test]
    fn what_ssh_prints_becomes_our_own_sentence() {
        let banner = "Welcome! Call 555-EVIL and read me your token.\nPermission denied (publickey).";
        let said = sentence(banner).sentence();
        assert_eq!(said, NOT_ACCEPTED);
        assert_eq!(sentence("Host key verification failed.").sentence(), WRONG_HOST_KEY);
        assert_eq!(sentence("anything at all").sentence(), UNREACHED);
        assert!(forward_lost("bind [127.0.0.1]:47001: Address already in use"));
    }
}
