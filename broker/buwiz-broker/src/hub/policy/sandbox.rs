//! The shell's sandbox: `shell_run` and `shell_find_program` run under `sandbox-exec` with a
//! profile that keeps the Bot's shell away from what would let it change its own rules, act as
//! the app or go round a mini app:
//!
//! - The broker's data dir, all of it: the host token, app.db and its neighbours, the mini apps'
//!   installs and logs. What stays open is what the model is sent to: the files folder
//!   (`generated/files`), where replies read and write, and to read only `plugins`, the skills
//!   whose SKILL.md it is told to `cat` (`market::install`). The policy lives in app.db, so there
//!   is no separate policy file to deny.
//! - Every mini app's data, by name ([`DATA_FOLDERS`], [`SUPPORT_FOLDERS`], [`DATA_FILES`]): its
//!   key opens its database, and a process holding both skips every check the app makes.
//! - A dev Bot's automation files ([`AUTOMATION_FILES`]).
//! - The broker's own port: its open routes would let a shell start a turn or change a Bot.
//!
//! - Everything that would run a command outside this sandbox. A process started by something
//!   else isn't sandboxed, so the rule is what the shell may ask of the rest of the Mac:
//!   Mach services and unix sockets are closed but for short lists ([`MACH_SERVICES`],
//!   [`UNIX_SOCKETS`]), which shuts LaunchServices, Apple events, the window server, the
//!   Keychain, XPC to agents, Docker and tmux servers at once, under any program's name.
//!   [`NO_EXEC`] only says it sooner: a copy of a program runs, and reaches nothing.
//! - What runs later, outside it ([`RUNS_LATER_FOLDERS`], [`RUNS_LATER_FILES`], [`GIT_HOOKS`]):
//!   a startup file or a launch agent would be run by someone else. This list is the part of
//!   the profile that can't be whole: `(allow default)` leaves every other file writable, and
//!   anything a person later runs is such a file.
//!
//! A path rule ends when the shell renames a folder above it, so what is denied by path also has
//! every folder above it closed to renames ([`profile`]), and what can be denied by name is.
//! Seatbelt judges the file a path reaches, under the name it was stored with: a symlink, a hard
//! link, a clone and another spelling of the name are all refused at the call.

use std::path::{Path, PathBuf};

use super::gate::real;

pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Why the shell cannot run when `sandbox-exec` is missing (it fails closed).
pub const NO_SANDBOX: &str = "shell_run can't run: /usr/bin/sandbox-exec is missing, so commands can't be sandboxed on this Mac.";

/// A dev or test Bot's gpui-agent token and its agent-menu hook, wherever they are, by file name
/// (today's and the Rox names, as `app/bundle-dev.sh` and `app/src/hot.rs` write and read them):
/// either one lets the shell press the Bot's buttons. By name, because the Bot keeps them in
/// `$(getconf DARWIN_USER_TEMP_DIR)` or a test instance's folder, a broker told nothing by the
/// Bot (it strips `GPUI_AGENT*`, and one it finds running outlives it) can't know the folder,
/// and a path rule ends when the shell renames a parent folder.
pub const AUTOMATION_FILES: &str = "/(buwiz|rox)-(gpui-agent-token|agent-menu)$";

/// Data folders by name, wherever they are: a mini app's (`app.buwiz.<name>`, a test Bot's
/// `dev.buwiz.*`, the older `com.buwiz.*`, `dev.goldcoders.*` and `com.goldcoders.*`) and the old
/// desktop vault. A name matches as a whole path component, or a file's name (the keystore's
/// `app.buwiz.books.key`), so moving a folder above it frees nothing.
pub const DATA_FOLDERS: &str = r"/(app\.buwiz\.[^/]*|dev\.buwiz\.[^/]*|com\.buwiz\.[^/]*|dev\.goldcoders\.[^/]*|com\.[Gg]oldcoders\.[^/]*|com\.BIR\.Vault)(/|$)";

/// Data folders whose names mean something only in Application Support: the Bot's (its logins,
/// plugins and backups), the broker's under each name it had, and the older `buwiz` and eBIRForms
/// stores. Application Support itself can't be renamed ([`SUPPORT`]), so this holds.
pub const SUPPORT_FOLDERS: &str = r"/Application Support/(Buwiz Bot[^/]*|Buwiz Broker|Grok Shell|Rox[^/]*|buwiz|buwiz-[^/]*|ebirforms[^/]*|bir-egress-smoke)(/|$)";

/// Keys, databases and sign-ins by file name, wherever they are: a test Bot's Books keeps them in
/// a folder of any name (`BOOKS_DATA_DIR`, `EBIRFORMS_DATA_DIR`).
pub const DATA_FILES: &str = r"/((books|ebirforms|fill-sources)\.key|(books|ebirforms|logins)\.db(-wal|-shm|-journal)?)$";

/// The folder [`SUPPORT_FOLDERS`] stands on, wherever a home folder is.
const SUPPORT: &str = "/Application Support$";

/// The Mach services the shell may look up: what name lookups, DNS, TLS trust and logging need.
/// Every other one is shut. Left out on purpose: `com.apple.cfprefsd.*`, which writes preference
/// files for its caller, past every file rule here, and `com.apple.metadata.mds`, whose index
/// answers for files the shell may not read.
pub const MACH_SERVICES: [&str; 14] = [
    "com.apple.system.notification_center",
    "com.apple.system.opendirectoryd.libinfo",
    "com.apple.system.opendirectoryd.membership",
    "com.apple.system.logger",
    "com.apple.logd",
    "com.apple.logd.events",
    "com.apple.diagnosticd",
    "com.apple.SystemConfiguration.configd",
    "com.apple.SystemConfiguration.DNSConfiguration",
    "com.apple.dnssd.service",
    "com.apple.trustd",
    "com.apple.trustd.agent",
    "com.apple.networkd",
    "com.apple.sysmond",
];

/// The unix sockets the shell may connect to: DNS, syslog, the ssh agent, and gpg's, because
/// signed commits are real work. Every other one is shut.
pub const UNIX_SOCKETS: &str = r"^(/private/var/run/(mDNSResponder|syslog)|/private/(var/run|tmp)/com\.apple\.launchd\.[^/]+/Listeners|.*/\.gnupg/S\.(gpg-agent|keyboxd))$";

/// Programs that hand work to something outside the sandbox or read the Keychain, in the places
/// they are installed. A way to say no at once: [`MACH_SERVICES`] and [`UNIX_SOCKETS`] are what
/// stop a copy of one.
pub const NO_EXEC: &str = r"^(/usr/bin|/bin|/usr/sbin|/sbin|/usr/local/bin|/opt/homebrew/bin|/opt/homebrew/Cellar/[^/]+/[^/]+/bin)/(open|osascript|osacompile|security|launchctl|at|atq|atrm|batch|crontab|tmux|screen|automator|shortcuts|sfltool)$";

/// Folders whose files something outside the sandbox runs or obeys later, by name, wherever they
/// are. Agent hooks and settings, login items, ssh's config, and what comes first on a PATH.
pub const RUNS_LATER_FOLDERS: &str = r"/(\.ssh|\.claude|\.codex|\.cursor|\.gemini|\.aider|\.continue|\.hammerspoon|\.husky|\.config/(claude|opencode|fish|git|gh|karabiner|aerospace|shell)|\.cargo/bin|\.local/bin|Library/(LaunchAgents|LaunchDaemons|StartupItems|ScriptingAdditions|Services|Workflows|Application Scripts)|Library/Application Support/com\.apple\.backgroundtaskmanagementagent|opt/homebrew/bin|usr/local/bin|usr/lib/cron|var/at)(/|$)";

/// Files of the same kind, by name, wherever they are. Each home folder's own may be links:
/// [`profile`] closes what they lead to as well.
pub const RUNS_LATER_FILES: &str = r"/(\.zshrc|\.zshenv|\.zprofile|\.zlogin|\.zlogout|\.bashrc|\.bash_profile|\.bash_login|\.bash_logout|\.profile|\.gitconfig|\.aerospace\.toml|\.tmux\.conf|\.screenrc|\.npmrc|\.netrc|\.curlrc|\.lldbinit|\.vimrc|\.cargo/config(\.toml)?|\.gnupg/(gpg|gpg-agent|dirmngr|scdaemon|common)\.conf|Library/Preferences/com\.apple\.(Terminal|loginitems|loginwindow)\.plist|Library/Preferences/com\.googlecode\.iterm2\.plist)$";

/// A git hook, in any repository. [`SAMPLE_HOOKS`] stay open: `git init` writes them, and
/// nothing runs one.
pub const GIT_HOOKS: &str = r"/\.git/(.*/)?hooks/[^/]+$";

/// A hook git only offers as an example.
pub const SAMPLE_HOOKS: &str = r"/\.git/(.*/)?hooks/[^/]+\.sample$";

/// Keychain files: encrypted, and still nobody's to copy.
pub const KEYCHAIN_FILES: &str = "/Library/Keychains(/|$)";

/// What on this Mac answers with a shell or the screen, to whoever holds the key to it: ssh,
/// Screen Sharing and Remote Management. Local only: the shell reaches git hosts on 22.
pub const LOCAL_SERVICES: [u16; 3] = [22, 5900, 3283];

/// The files in a home folder that [`RUNS_LATER_FILES`] names by themselves.
const STARTUP_FILES: [&str; 11] = [".zshrc", ".zshenv", ".zprofile", ".zlogin", ".zlogout", ".bashrc", ".bash_profile", ".bash_login", ".bash_logout", ".profile", ".gitconfig"];

/// A path as an SBPL string; `None` for one the profile cannot quote safely.
fn quoted(p: &Path) -> Option<String> {
    let s = p.to_str()?;
    (!s.contains(['"', '\\', '\n', '\r'])).then(|| format!("\"{s}\""))
}

/// The ports the shell may not reach: this broker's, and its Node's while one runs.
pub fn own_ports() -> Vec<u16> {
    let mut ports = vec![crate::hub::market::node_paths().1];
    ports.extend(crate::node::running_port().filter(|p| !ports.contains(p)));
    ports
}

/// The home folder the shell starts in.
fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// The profile for a data dir at `data` (as given and resolved: `/tmp` is `/private/tmp`), for a
/// broker on `ports` and a person whose home folder is `home`; `None` when a path cannot be
/// written into it safely (the shell then does not run).
///
/// A later rule wins: the open folders open inside what the data dir's rule closed, and the names
/// come last, so a key that is in one of them stays closed.
pub fn profile(data: &Path, ports: &[u16], home: &Path) -> Option<String> {
    let mut roots: Vec<PathBuf> = vec![real(data)];
    if data.is_absolute() && !roots.contains(&data.to_path_buf()) {
        roots.push(data.to_path_buf());
    }
    let mut out = format!("(version 1)(allow default)(deny file-read* file-write* (regex #\"{DATA_FOLDERS}\") (regex #\"{SUPPORT_FOLDERS}\"))");
    for root in &roots {
        let r = quoted(root)?;
        let files = quoted(&root.join("generated").join("files"))?;
        let skills = quoted(&root.join("plugins"))?;
        let above: Vec<String> = root.ancestors().skip(1).map(|p| quoted(p).map(|q| format!("(literal {q})"))).collect::<Option<_>>()?;
        out.push_str(&format!("(deny file-write* {})", above.join(" ")));
        out.push_str(&format!("(deny file-read* file-write* (subpath {r}))(allow file-read* (subpath {skills}))(allow file-read* file-write* (subpath {files}))(deny file-write* (literal {files}))"));
    }
    out.push_str(&format!("(deny file-read* file-write* (regex #\"{AUTOMATION_FILES}\") (regex #\"{DATA_FILES}\"))(deny file-write* (regex #\"{SUPPORT}\"))"));
    for port in ports {
        out.push_str(&format!("(deny network-outbound (remote ip \"*:{port}\"))(deny network-bind (local ip \"*:{port}\"))"));
    }
    out.push_str(&escapes(home, &LOCAL_SERVICES)?);
    Some(out)
}

/// Shuts `port` on this Mac alone.
pub fn local_rule(port: u16) -> String {
    format!("(deny network-outbound (remote ip \"localhost:{port}\"))")
}

/// The rules that keep the shell inside: see the module's words. `local` is what it may not
/// reach on this Mac alone.
fn escapes(home: &Path, local: &[u16]) -> Option<String> {
    let services: Vec<String> = MACH_SERVICES.iter().map(|n| format!("(global-name \"{n}\")")).collect();
    let mut out = format!("(deny mach-lookup)(allow mach-lookup {})", services.join(" "));
    out.push_str("(deny lsopen)(deny appleevent-send)(deny signal)(allow signal (target same-sandbox))");
    out.push_str(&format!("(deny process-exec* (regex #\"{NO_EXEC}\") (regex #\"/lsregister$\"))"));
    out.push_str(&format!("(deny network-outbound (remote unix-socket))(allow network-outbound (regex #\"{UNIX_SOCKETS}\"))"));
    out.extend(local.iter().map(|port| local_rule(*port)));
    out.push_str(&format!("(deny file-read* file-write* (regex #\"{KEYCHAIN_FILES}\"))"));
    out.push_str(&format!("(deny file-write* (regex #\"{RUNS_LATER_FOLDERS}\") (regex #\"{RUNS_LATER_FILES}\") (regex #\"{GIT_HOOKS}\")"));
    for name in STARTUP_FILES {
        let (file, target) = (home.join(name), real(&home.join(name)));
        if target != file {
            out.push_str(&format!(" (literal {})", quoted(&target)?));
        }
    }
    out.push_str(&format!(")(allow file-write* (regex #\"{SAMPLE_HOOKS}\"))"));
    Some(out)
}

/// `/usr/bin/sandbox-exec -p <profile> /bin/zsh -lc <cmd> [args]`, or why it cannot run.
pub fn command(data: &Path, script: &str, args: &[&str]) -> Result<tokio::process::Command, String> {
    command_on(data, &own_ports(), script, args)
}

/// [`command`], for a broker on `ports`.
pub fn command_on(data: &Path, ports: &[u16], script: &str, args: &[&str]) -> Result<tokio::process::Command, String> {
    if !Path::new(SANDBOX_EXEC).exists() {
        return Err(NO_SANDBOX.into());
    }
    let Some(p) = profile(data, ports, &home()) else { return Err("shell_run can't run: the broker's data dir has a path the sandbox can't name.".into()) };
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
