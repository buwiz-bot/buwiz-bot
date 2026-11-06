//! Rox starts its broker (plan 7.2, decision O6; no LaunchAgent). At
//! launch, when nothing answers on the broker's default origin
//! ([`client::DEFAULT_GEN`]), Rox runs the `buwiz-broker` its own bundle
//! carries (`Contents/Resources/broker/`, put there by `bundle-dev.sh`):
//! the hub on, hosting the v1 plugins and MCP servers, no Node, on the
//! owner's broker folder (`~/Library/Application Support/Buwiz Broker`,
//! moved there from `Grok Shell` once, just before this start:
//! [`move_data_dir`]). It runs detached, in its own process
//! group, so it outlives Rox; its log is `~/Library/Logs/Buwiz Bot/broker.log`.
//!
//! Never started:
//! - when something answers already (the owner's broker, run today by
//!   `scripts/broker-live`, is simply found);
//! - by a test instance: `BUWIZ_BACKEND` set (it names its own broker),
//!   `BUWIZ_BACKGROUND=1` or `BUWIZ_DATA_DIR` set (what `scripts/buwiz-test`
//!   sets);
//! - by a build outside a bundle (no `buwiz-broker` beside it);
//! - while another Rox holds the start lock (`<Rox data dir>/broker.lock`,
//!   held until the broker it started answers). The broker binds its port
//!   before anything else, so a second one exits at once anyway.
//!
//! [`client::DEFAULT_GEN`]: crate::client::DEFAULT_GEN

use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// What the broker's origin answered to `GET /health`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// 200.
    Healthy,
    /// Something holds the port but is not healthy (a broker still
    /// starting, or another program): a new broker could not bind it.
    Unhealthy,
    /// Nothing listens.
    Nothing,
}

/// Whether this Rox starts the broker, and why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Start,
    /// a test instance never starts one
    TestInstance,
    /// the origin answers already
    Answering(Answer),
    /// no `buwiz-broker` in this Rox's bundle (a bare build)
    NotBundled,
    /// another Rox holds the start lock: it is starting one
    LockHeld,
}

/// The port of an origin, as a connection would use it (`08787` is 8787).
fn port_of(origin: &str) -> Option<u32> {
    let (default, rest) = match origin.split_once("://") {
        Some(("https", rest)) => (443, rest),
        Some((_, rest)) => (80, rest),
        None => (80, origin),
    };
    let host = rest.rsplit('@').next()?;
    let after = host.rsplit_once(']').map_or(host, |(_, port)| port);
    match after.rsplit_once(':') {
        Some((_, port)) => port.parse().ok(),
        None => Some(default),
    }
}

/// `path` with `.` and `..` worked out, and links followed where it exists.
fn resolved(path: &Path) -> PathBuf {
    if let Ok(real) = path.canonicalize() {
        return real;
    }
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether `path` is in a folder of the owner's, under `home`.
fn is_the_owners(path: &Path, home: &Path) -> bool {
    use buwiz_protocol::broker_dir::{new_dir, old_dir};
    let support = home.join("Library/Application Support");
    let owners = [
        support.join(crate::identity::DIR_NAME),
        support.join(crate::identity::OLD_DIR_NAME),
        support.join(crate::identity::APP_ID),
        support.join(crate::identity::OLD_APP_ID),
        new_dir(home),
        old_dir(home),
    ];
    let path = resolved(path);
    owners.iter().any(|o| path.starts_with(resolved(o)))
}

/// Whether this binary is a test bundle's: by its name, or the id of the bundle
/// it runs in. That id comes from the bundle, not the environment, which a child
/// would inherit.
pub fn is_test_bundle(exe: Option<&str>, bundle: Option<&str>) -> bool {
    exe.is_some_and(|e| e.starts_with("buwiz-bot-test-"))
        || bundle.is_some_and(|b| b.starts_with("dev.buwiz.bot.test-"))
}

/// Why this test bundle must not run, if it mustn't. `scripts/buwiz-test` copies
/// the Bot into a bundle of its own; started any other way, by Finder, `open` or
/// an automation tool, that copy has none of its settings, and would take itself
/// for the owner's Bot: it would find the owner's broker on the default origin,
/// read their chats and write to them. So it has to name a broker and a data
/// folder that aren't the owner's, or it doesn't start.
///
/// What's judged is what would be used: the variables as the app reads them,
/// through the same functions.
pub fn test_bundle_refusal(
    exe: Option<&str>,
    bundle: Option<&str>,
    get: impl Fn(&str) -> Option<OsString>,
) -> Option<String> {
    if !is_test_bundle(exe, bundle) {
        return None;
    }
    let read = |name: &str| buwiz_protocol::env::var_in(name, &get);
    let text = |name: &str| read(name).map(|v| v.to_string_lossy().trim().to_owned());
    let home = PathBuf::from(get("HOME").unwrap_or_default());

    let origin = crate::client::backend_from(text("BUWIZ_BACKEND").as_deref());
    let owners = port_of(crate::client::DEFAULT_GEN);
    if port_of(&origin).is_none() || port_of(&origin) == owners {
        return Some(format!(
            "this test bundle would use {origin}, the owner's broker: not starting; use scripts/buwiz-test"
        ));
    }
    let data = text("BUWIZ_DATA_DIR").filter(|d| !d.is_empty());
    let Some(data) = data else {
        return Some("this test bundle has no data folder of its own (BUWIZ_DATA_DIR): not starting; use scripts/buwiz-test".to_owned());
    };
    if is_the_owners(Path::new(&data), &home) {
        return Some("BUWIZ_DATA_DIR is the owner's folder: not starting a test bundle on it".to_owned());
    }
    let token = text("BUWIZ_HOST_TOKEN_FILE").filter(|t| !t.is_empty());
    if token.is_some_and(|t| is_the_owners(Path::new(&t), &home)) {
        return Some("BUWIZ_HOST_TOKEN_FILE is the owner's: not starting a test bundle on it".to_owned());
    }
    None
}

/// This binary's name.
fn exe_name() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    exe.file_name()?.to_str().map(str::to_owned)
}

/// Whether this process is a test bundle's.
pub fn in_test_bundle() -> bool {
    is_test_bundle(exe_name().as_deref(), crate::bundle_id().as_deref())
}

/// [`test_bundle_refusal`] for this process.
pub fn refusal() -> Option<String> {
    test_bundle_refusal(exe_name().as_deref(), crate::bundle_id().as_deref(), |k| std::env::var_os(k))
}

/// A test instance: `BUWIZ_BACKEND` set at all (it names its own broker;
/// even empty, the owner's Rox never sets it), `BUWIZ_BACKGROUND=1`, or
/// `BUWIZ_DATA_DIR` set (`scripts/buwiz-test` sets it for every instance).
pub fn is_test_instance(backend: Option<&str>, background: Option<&str>, data_dir: Option<&str>) -> bool {
    backend.is_some() || background == Some("1") || data_dir.is_some_and(|d| !d.is_empty())
}

/// [`is_test_instance`] over the environment `get` reads, by the Buwiz
/// names and, apart, by their old ones (`ROX_BACKEND`, `ROX_BACKGROUND`,
/// `ROX_DATA_DIR`): a bench that sets either is a test instance, so it never
/// starts a broker on the owner's port or data.
pub fn test_instance_in(get: impl Fn(&str) -> Option<OsString>) -> bool {
    let s = |k: &str| get(k).map(|v| v.to_string_lossy().into_owned());
    let by = |names: [&str; 3]| is_test_instance(s(names[0]).as_deref(), s(names[1]).as_deref(), s(names[2]).as_deref());
    let new = ["BUWIZ_BACKEND", "BUWIZ_BACKGROUND", "BUWIZ_DATA_DIR"];
    let test = by(new) || by(new.map(|n| buwiz_protocol::env::old_name(n).unwrap_or(n)));
    // the old names' warning, once
    for n in new {
        let _ = buwiz_protocol::env::var_in(n, &get);
    }
    test
}

/// The start decision. `probe` asks the origin (twice when it starts: a
/// broker another Rox started between the first probe and the lock
/// answers by then); `lock` takes the start lock, true when this Rox got
/// it. Neither runs for a test instance, and the lock is taken only when
/// nothing answers and there is a broker to start.
pub fn decide(test: bool, mut probe: impl FnMut() -> Answer, bundled: bool, lock: impl FnOnce() -> bool) -> Decision {
    if test {
        return Decision::TestInstance;
    }
    match probe() {
        Answer::Nothing => {}
        a => return Decision::Answering(a),
    }
    if !bundled {
        return Decision::NotBundled;
    }
    if !lock() {
        return Decision::LockHeld;
    }
    match probe() {
        Answer::Nothing => Decision::Start,
        a => Decision::Answering(a),
    }
}

/// `/health`'s answer from the first bytes of the reply.
pub fn answer_of(head: &[u8]) -> Answer {
    if head.starts_with(b"HTTP/1.1 200") || head.starts_with(b"HTTP/1.0 200") {
        Answer::Healthy
    } else {
        Answer::Unhealthy
    }
}

/// The default origin's address (`127.0.0.1:8787`).
pub fn default_addr() -> SocketAddr {
    let host = crate::client::DEFAULT_GEN.trim_start_matches("http://");
    host.parse().unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], 8787)))
}

/// `GET /health` on `addr`, with short timeouts.
fn probe(addr: SocketAddr) -> Answer {
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
        return Answer::Nothing;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let req = format!("GET /health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    if s.write_all(req.as_bytes()).is_err() {
        return Answer::Unhealthy;
    }
    let mut head = [0u8; 12];
    match s.read_exact(&mut head) {
        Ok(()) => answer_of(&head),
        Err(_) => Answer::Unhealthy,
    }
}

/// The bundled broker's file name, and its name before the rename (an app
/// binary updated in a bundle `bundle-dev.sh` has not repackaged yet).
pub const BROKER_NAME: &str = "buwiz-broker";
pub const OLD_BROKER_NAME: &str = "rox-broker";

/// The broker in the bundle `exe` runs from (`<app>/Contents/MacOS/<exe>`
/// → `<app>/Contents/Resources/broker/buwiz-broker`, else `…/rox-broker`
/// when only that one `is_file`); `None` outside a bundle or with neither.
pub fn bundled_broker(exe: &Path, is_file: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let shape = macos.file_name()? == "MacOS" && contents.file_name()? == "Contents";
    let dir = shape.then(|| contents.join("Resources").join("broker"))?;
    [BROKER_NAME, OLD_BROKER_NAME].iter().map(|n| dir.join(n)).find(|b| is_file(b))
}

/// How the broker is started.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(&'static str, OsString)>,
    /// Rox's own variables the broker and its plugins must not inherit
    /// (the automation token, a port meant for Rox's tests).
    pub env_remove: Vec<&'static str>,
    pub data: PathBuf,
    pub log: PathBuf,
}

/// The broker's data dir: `Buwiz Broker`, else `Grok Shell` while it was
/// not moved yet (`buwiz_protocol::broker_dir::dir_in`).
pub fn data_dir(home: &Path) -> PathBuf {
    buwiz_protocol::broker_dir::dir_in(home)
}

/// Moves the broker's folder from `Grok Shell` to `Buwiz Broker` once,
/// after a backup of its database, rewriting the paths stored in it and
/// leaving `Grok Shell` as a link (`buwiz_protocol::broker_dir::move_once`, the
/// one the broker runs too). Called only when this app is about to start
/// the broker: nothing answered on the owner's port, and `busy` asks once
/// more. A move that can't run leaves `Grok Shell` in use ([`data_dir`])
/// and is tried at the next start.
pub fn move_data_dir(home: &Path, busy: &dyn Fn() -> bool) -> buwiz_protocol::broker_dir::Move {
    use buwiz_protocol::broker_dir::{Move, DIR_NAME, OLD_DIR_NAME};
    let m = buwiz_protocol::broker_dir::move_once(home, busy);
    match &m {
        Move::Nothing | Move::AlreadyThere => {}
        Move::Moved { backup, rewritten, linked } => {
            log::info!("broker: moved {OLD_DIR_NAME} to {DIR_NAME} (database backup: {backup:?}; {OLD_DIR_NAME} left as a link to it: {linked})");
            for (column, n) in rewritten {
                log::info!("broker: {column}: {n} stored path(s) now under {DIR_NAME}");
            }
        }
        other => log::warn!("broker: {OLD_DIR_NAME} not moved to {DIR_NAME} this time ({other:?}); using it"),
    }
    m
}


/// The command line for `broker` (the bundle's `buwiz-broker`): the default
/// origin's port, the data dir ([`data_dir`]), the static files beside it
/// (`broker/server`), no Node; the hub on, hosting v1 plugins and MCP.
/// `nsh` and `nsh-macos-host` are the bundle's (`broker/bin/`, as Grok
/// Shell.app set them): a Finder-launched Rox has no nsh on PATH. The
/// widget pipeline runs `nsh build`/`check`, pop-outs `nsh-macos-host`,
/// which loads generated modules from `NSH_CHUNK_DIR`.
pub fn launch(broker: &Path, home: &Path, port: u16) -> Launch {
    let data = data_dir(home);
    let dir = broker.parent().unwrap_or(Path::new("."));
    let assets = dir.join("server");
    let args = vec![
        "--port".into(),
        port.to_string().into(),
        "--data".into(),
        data.clone().into(),
        "--assets".into(),
        assets.into(),
        "--no-node".into(),
    ];
    // by the Buwiz names and the old ones: the bundle's broker may be an
    // older build that reads only the old names
    let both = |k: &'static str, v: OsString| buwiz_protocol::env::with_old(k).map(move |n| (n, v.clone()));
    let env = both("BUWIZ_BROKER_HUB", "1".into())
        .chain(both("BUWIZ_BROKER_V1_HOST", "1".into()))
        .chain(both("BUWIZ_BROKER_MCP_HOST", "1".into()))
        .chain(both("BUWIZ_BROKER_DATA_DIR", data.clone().into()))
        .chain([
            ("NSH", dir.join("bin").join("nsh").into()),
            ("NSH_HOST", dir.join("bin").join("nsh-macos-host").into()),
            ("NSH_CHUNK_DIR", data.join("chunks").into()),
        ])
        .collect();
    let mut env_remove = vec!["GPUI_AGENT", "GPUI_AGENT_TOKEN", "GPUI_AGENT_TOKEN_FILE", "GPUI_AGENT_ADDR", "GPUI_AGENT_ALLOW_RELEASE", "PORT"];
    // a release binary doesn't name it: `scripts/release/build.sh` checks
    #[cfg(feature = "dev-agent")]
    env_remove.push("GPUI_AGENT_ACTOR");
    env_remove.extend(buwiz_protocol::env::with_old("BUWIZ_PUBLIC_PORT"));
    let log = crate::identity::logs_dir(home).join("broker.log");
    Launch { program: broker.to_owned(), args, env, env_remove, data, log }
}

/// Starts `l` detached: its own process group, stdin closed, output
/// appended to its log, working in `/`. A thread reaps it if it exits
/// while Rox runs; it outlives Rox otherwise.
fn spawn(l: &Launch) -> std::io::Result<u32> {
    use std::os::unix::process::CommandExt;
    if let Some(dir) = l.log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::create_dir_all(&l.data)?;
    let log = OpenOptions::new().create(true).append(true).open(&l.log)?;
    let mut cmd = std::process::Command::new(&l.program);
    cmd.args(&l.args)
        .envs(l.env.iter().map(|(k, v)| (*k, v)))
        .current_dir("/")
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0);
    for k in &l.env_remove {
        cmd.env_remove(k);
    }
    let mut child = cmd.spawn()?;
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}

/// The start lock: held while this Rox starts the broker. `Ok(None)` when
/// another holds it; an error when it can't be taken at all.
fn try_lock(path: &Path) -> std::io::Result<Option<File>> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let f = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
    match f.try_lock() {
        Ok(()) => Ok(Some(f)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

/// Decides and, when it should, starts the broker and waits (up to 20 s)
/// for it to answer before letting the lock go.
fn run() -> Decision {
    let test = test_instance_in(|k| std::env::var_os(k));
    let addr = default_addr();
    let broker = std::env::current_exe().ok().and_then(|e| bundled_broker(&e, |b| b.is_file()));
    let mut held = None;
    let decision = decide(test, || probe(addr), broker.is_some(), || {
        match try_lock(&crate::vault::data_dir().join("broker.lock")) {
            Ok(Some(f)) => {
                held = Some(f);
                true
            }
            Ok(None) => false,
            // no lock to be had: the broker's own bind keeps it to one
            Err(e) => {
                log::warn!("broker: no start lock ({e}); starting anyway");
                true
            }
        }
    });
    let (Decision::Start, Some(broker)) = (decision, broker) else {
        return decision;
    };
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    // nothing answers and this app holds the start lock: no broker uses the
    // folder, so it can move before the broker opens it
    move_data_dir(&home, &|| probe(addr) != Answer::Nothing);
    let l = launch(&broker, &home, addr.port());
    match spawn(&l) {
        Ok(pid) => log::info!("broker: started {} (pid {pid}), log {}", l.program.display(), l.log.display()),
        Err(e) => {
            log::error!("broker: could not start {}: {e}", l.program.display());
            return decision;
        }
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    while probe(addr) == Answer::Nothing && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    drop(held);
    decision
}

/// At launch, off the main thread: start the bundled broker when none
/// answers ([`decide`]).
pub fn ensure_started() {
    let _ = std::thread::Builder::new().name("buwiz-broker-start".into()).spawn(|| {
        let d = run();
        log::info!("broker: {d:?}");
    });
}

#[cfg(test)]
mod tests {
    mod test_bundles {
        use super::super::{is_the_owners, port_of, test_bundle_refusal};
        use std::ffi::OsString;
        use std::path::Path;

        const HOME: &str = "/Users/u";
        const EXE: Option<&str> = Some("buwiz-bot-test-gws");
        const BROKER: (&str, &str) = ("BUWIZ_BACKEND", "http://127.0.0.1:8821");
        const DATA: (&str, &str) = ("BUWIZ_DATA_DIR", "/tmp/x/data");

        fn refused(exe: Option<&str>, bundle: Option<&str>, vars: &[(&str, &str)]) -> Option<String> {
            let mut vars: Vec<(String, String)> = vars.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
            vars.push(("HOME".into(), HOME.into()));
            test_bundle_refusal(exe, bundle, move |k| {
                vars.iter().find(|(n, _)| n == k).map(|(_, v)| OsString::from(v))
            })
        }

        #[test]
        fn a_test_bundle_started_bare_refuses() {
            assert!(refused(EXE, None, &[]).is_some());
            assert!(refused(None, Some("dev.buwiz.bot.test-gws"), &[]).is_some());
        }

        #[test]
        fn half_of_a_test_setup_refuses_too() {
            assert!(refused(EXE, None, &[("BUWIZ_BACKGROUND", "1")]).is_some());
            assert!(refused(EXE, None, &[DATA]).is_some(), "the client falls back to the owner's");
            assert!(refused(EXE, None, &[BROKER]).is_some());
        }

        /// Every way to write the owner's broker, and every way to name none.
        #[test]
        fn what_counts_is_the_broker_the_client_would_use() {
            for backend in [
                "http://127.0.0.1:8787",
                "http://127.0.0.1:8787/",
                "http://127.0.0.1:08787",
                "http://localhost:8787",
                "https://127.0.0.1:8787",
                "http://[::1]:8787",
                "",
                "   ",
            ] {
                assert!(refused(EXE, None, &[("BUWIZ_BACKEND", backend), DATA]).is_some(), "{backend:?}");
            }
            // a blank new name shadows a good old one, and the client goes to the default
            let shadow = [("BUWIZ_BACKEND", "   "), ("ROX_BACKEND", "http://127.0.0.1:8821"), DATA];
            assert!(refused(EXE, None, &shadow).is_some());
        }

        #[test]
        fn ports_read_as_a_connection_reads_them() {
            assert_eq!(port_of("http://127.0.0.1:08787"), Some(8787));
            assert_eq!(port_of("http://user@a.example:8787"), Some(8787));
            assert_eq!(port_of("http://[::1]:8787"), Some(8787));
            assert_eq!(port_of("http://a.example"), Some(80));
            assert_eq!(port_of("https://a.example"), Some(443));
            assert_eq!(port_of("http://a.example:x"), None);
        }

        #[test]
        fn the_owners_folders_are_refused_by_any_name() {
            for dir in [
                "/Users/u/Library/Application Support/Buwiz Bot",
                "/Users/u/Library/Application Support/Buwiz Bot/",
                "/Users/u/Library/Application Support/Rox",
                "/Users/u/Library/Application Support/com.buwiz.bot",
                "/Users/u/Library/Application Support/dev.goldcoders.rox",
                "/Users/u/Library/Application Support/Buwiz Broker",
                "/Users/u/Library/Application Support/Grok Shell/x",
                "/tmp/../Users/u/Library/Application Support/Buwiz Bot/sub",
            ] {
                assert!(is_the_owners(Path::new(dir), Path::new(HOME)), "{dir}");
                assert!(refused(EXE, None, &[BROKER, ("BUWIZ_DATA_DIR", dir)]).is_some(), "{dir}");
            }
            assert!(refused(EXE, None, &[BROKER, ("BUWIZ_DATA_DIR", "   ")]).is_some());
        }

        #[test]
        fn the_owners_token_file_is_refused() {
            let token = ("BUWIZ_HOST_TOKEN_FILE", "/Users/u/Library/Application Support/Buwiz Broker/host-token");
            assert!(refused(EXE, None, &[BROKER, DATA, token]).is_some());
            let own = ("BUWIZ_HOST_TOKEN_FILE", "/tmp/x/broker/host-token");
            assert_eq!(refused(EXE, None, &[BROKER, DATA, own]), None);
        }

        #[test]
        fn a_test_bundle_with_its_own_setup_starts() {
            assert_eq!(refused(EXE, Some("dev.buwiz.bot.test-gws"), &[BROKER, DATA]), None);
            let old = [("ROX_BACKEND", "http://127.0.0.1:8821"), ("ROX_DATA_DIR", "/tmp/x")];
            assert_eq!(refused(EXE, None, &old), None);
            let nobody = [("BUWIZ_BACKEND", "http://127.0.0.1:1"), DATA];
            assert_eq!(refused(EXE, None, &nobody), None);
            // no scheme is a typo: the client goes to an address nothing answers on
            let typo = [("BUWIZ_BACKEND", "127.0.0.1:8787"), DATA];
            assert_eq!(refused(EXE, None, &typo), None);
        }

        #[test]
        fn the_owners_bot_is_never_refused() {
            assert_eq!(refused(Some("buwiz-bot"), Some("com.buwiz.bot"), &[]), None);
            assert_eq!(refused(Some("Buwiz Bot"), None, &[]), None);
            assert_eq!(refused(None, None, &[]), None);
        }

        /// The macOS `main` is the only way in, and the check is its first
        /// statement: nothing it could reach a broker or show a window with
        /// comes first, on any path.
        #[test]
        fn the_check_is_the_first_thing_main_does() {
            let source = include_str!("main.rs");
            let mains: Vec<&str> = source.match_indices("fn main()").map(|(at, _)| &source[at..]).collect();
            assert_eq!(mains.len(), 3, "macOS, iOS, elsewhere");
            let macos = source.find("#[cfg(target_os = \"macos\")]\nfn main()").expect("the macOS main");
            let body = &source[macos..];
            let body = &body[body.find('{').unwrap() + 1..];
            let first = body.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("//")).unwrap();
            assert_eq!(first, "if let Some(why) = buwiz_bot::broker::refusal() {");
            assert!(!source.contains("#[ctor"), "nothing runs before main");
        }
    }

    use super::*;
    use std::cell::Cell;

    #[test]
    fn a_test_instance_is_one_with_its_own_backend_background_or_data_dir() {
        assert!(!is_test_instance(None, None, None));
        assert!(is_test_instance(Some("http://127.0.0.1:8798"), None, None));
        assert!(is_test_instance(Some(""), None, None));
        assert!(is_test_instance(None, Some("1"), None));
        assert!(!is_test_instance(None, Some("0"), None));
        assert!(is_test_instance(None, None, Some("/tmp/r/data")));
        assert!(!is_test_instance(None, None, Some("")));
    }

    /// A bench that still sets the old names is a test instance, as is one
    /// that sets the new; the owner's app (neither) is not.
    #[test]
    fn a_test_instance_by_its_old_or_new_names() {
        let env = |pairs: &[(&str, &str)]| {
            let pairs: Vec<(String, OsString)> = pairs.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
            move |k: &str| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
        };
        assert!(!test_instance_in(env(&[])));
        assert!(!test_instance_in(env(&[("HOME", "/Users/u"), ("BUWIZ_BACKGROUND", "0")])));
        for (new, old, v) in [("BUWIZ_BACKEND", "ROX_BACKEND", "http://127.0.0.1:8798"), ("BUWIZ_BACKGROUND", "ROX_BACKGROUND", "1"), ("BUWIZ_DATA_DIR", "ROX_DATA_DIR", "/tmp/r/data")] {
            assert!(test_instance_in(env(&[(new, v)])), "{new}");
            assert!(test_instance_in(env(&[(old, v)])), "{old}");
        }
        // an old name says so even when the new one says otherwise
        assert!(test_instance_in(env(&[("BUWIZ_BACKGROUND", "0"), ("ROX_BACKGROUND", "1")])));
        assert!(test_instance_in(env(&[("BUWIZ_DATA_DIR", ""), ("ROX_DATA_DIR", "/tmp/r/data")])));
    }

    /// Runs [`decide`] against canned answers; → (decision, probes made,
    /// whether the lock was asked for).
    fn run_decide(test: bool, answers: &[Answer], bundled: bool, lock_free: bool) -> (Decision, usize, bool) {
        let probes = Cell::new(0);
        let asked = Cell::new(false);
        let d = decide(
            test,
            || {
                let a = answers[probes.get()];
                probes.set(probes.get() + 1);
                a
            },
            bundled,
            || {
                asked.set(true);
                lock_free
            },
        );
        (d, probes.get(), asked.get())
    }

    #[test]
    fn nothing_answering_starts_one() {
        use Answer::*;
        assert_eq!(run_decide(false, &[Nothing, Nothing], true, true), (Decision::Start, 2, true));
    }

    #[test]
    fn an_answering_broker_is_used_and_no_lock_is_taken() {
        use Answer::*;
        assert_eq!(run_decide(false, &[Healthy], true, true), (Decision::Answering(Healthy), 1, false));
        // a broker still starting holds the port: another could not bind it
        assert_eq!(run_decide(false, &[Unhealthy], true, true), (Decision::Answering(Unhealthy), 1, false));
    }

    #[test]
    fn a_test_instance_never_probes_or_locks() {
        use Answer::*;
        assert_eq!(run_decide(true, &[Nothing, Nothing], true, true), (Decision::TestInstance, 0, false));
    }

    #[test]
    fn a_held_lock_means_another_rox_is_starting_it() {
        use Answer::*;
        assert_eq!(run_decide(false, &[Nothing, Nothing], true, false), (Decision::LockHeld, 1, true));
    }

    #[test]
    fn a_broker_started_by_another_rox_before_the_lock_is_found() {
        use Answer::*;
        assert_eq!(run_decide(false, &[Nothing, Healthy], true, true), (Decision::Answering(Healthy), 2, true));
    }

    #[test]
    fn a_build_outside_a_bundle_starts_none() {
        use Answer::*;
        assert_eq!(run_decide(false, &[Nothing], false, true), (Decision::NotBundled, 1, false));
    }

    #[test]
    fn health_is_a_200() {
        assert_eq!(answer_of(b"HTTP/1.1 200 OK\r\n"), Answer::Healthy);
        assert_eq!(answer_of(b"HTTP/1.0 200 OK\r\n"), Answer::Healthy);
        assert_eq!(answer_of(b"HTTP/1.1 503 Service Unavailable"), Answer::Unhealthy);
        assert_eq!(answer_of(b"SSH-2.0-x"), Answer::Unhealthy);
    }

    #[test]
    fn the_default_origin_is_the_live_port() {
        assert_eq!(default_addr(), SocketAddr::from(([127, 0, 0, 1], 8787)));
    }

    #[test]
    fn the_broker_is_in_the_bundles_resources() {
        let any = |_: &Path| true;
        assert_eq!(
            bundled_broker(Path::new("/A/Rox.app/Contents/MacOS/rox-bin"), any),
            Some(PathBuf::from("/A/Rox.app/Contents/Resources/broker/buwiz-broker"))
        );
        // a bundle packaged before the rename: its broker has the old name
        let old = |b: &Path| b.ends_with("broker/rox-broker");
        assert_eq!(
            bundled_broker(Path::new("/A/Rox.app/Contents/MacOS/rox-bin"), old),
            Some(PathBuf::from("/A/Rox.app/Contents/Resources/broker/rox-broker"))
        );
        assert_eq!(bundled_broker(Path::new("/A/Rox.app/Contents/MacOS/rox-bin"), |_: &Path| false), None);
        assert_eq!(bundled_broker(Path::new("/r/app/target/debug/buwiz-bot"), any), None);
        assert_eq!(bundled_broker(Path::new("/rox"), any), None);
    }

    #[test]
    fn the_command_line_runs_the_hub_without_node_on_the_owners_data_dir() {
        let b = Path::new("/A/Rox.app/Contents/Resources/broker/buwiz-broker");
        let l = launch(b, Path::new("/Users/u"), 8787);
        let data = "/Users/u/Library/Application Support/Buwiz Broker";
        assert_eq!(l.program, PathBuf::from(b));
        let args: Vec<&str> = l.args.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "--port",
                "8787",
                "--data",
                data,
                "--assets",
                "/A/Rox.app/Contents/Resources/broker/server",
                "--no-node"
            ]
        );
        let env: Vec<(&str, &str)> = l.env.iter().map(|(k, v)| (*k, v.to_str().unwrap())).collect();
        assert_eq!(
            env,
            [
                ("BUWIZ_BROKER_HUB", "1"),
                ("ROX_BROKER_HUB", "1"),
                ("BUWIZ_BROKER_V1_HOST", "1"),
                ("ROX_BROKER_V1_HOST", "1"),
                ("BUWIZ_BROKER_MCP_HOST", "1"),
                ("ROX_BROKER_MCP_HOST", "1"),
                ("BUWIZ_BROKER_DATA_DIR", data),
                ("GROK_DATA_DIR", data),
                ("NSH", "/A/Rox.app/Contents/Resources/broker/bin/nsh"),
                ("NSH_HOST", "/A/Rox.app/Contents/Resources/broker/bin/nsh-macos-host"),
                ("NSH_CHUNK_DIR", "/Users/u/Library/Application Support/Buwiz Broker/chunks")
            ]
        );
        assert!(l.env_remove.contains(&"GPUI_AGENT_TOKEN"));
        assert!(l.env_remove.contains(&"GPUI_AGENT_TOKEN_FILE"));
        assert!(l.env_remove.contains(&"BUWIZ_PUBLIC_PORT") && l.env_remove.contains(&"ROX_PUBLIC_PORT"));
        assert_eq!(l.data, PathBuf::from(data));
        assert_eq!(l.log, PathBuf::from("/Users/u/Library/Logs/Buwiz Bot/broker.log"));
    }

    #[test]
    fn nsh_is_the_bundles_and_chunks_go_to_the_data_dir() {
        let l = launch(Path::new("/B/Rox.app/Contents/Resources/broker/buwiz-broker"), Path::new("/Users/v"), 8800);
        let get = |k: &str| l.env.iter().find(|(n, _)| *n == k).map(|(_, v)| PathBuf::from(v));
        assert_eq!(get("NSH"), Some(PathBuf::from("/B/Rox.app/Contents/Resources/broker/bin/nsh")));
        assert_eq!(get("NSH_HOST"), Some(PathBuf::from("/B/Rox.app/Contents/Resources/broker/bin/nsh-macos-host")));
        assert_eq!(get("NSH_CHUNK_DIR"), Some(l.data.join("chunks")));
    }

    #[test]
    fn the_start_lock_admits_one() {
        let dir = std::env::temp_dir().join(format!("buwiz-broker-lock-{}", std::process::id()));
        let path = dir.join("broker.lock");
        let first = try_lock(&path).unwrap();
        assert!(first.is_some());
        assert!(try_lock(&path).unwrap().is_none(), "a second taker waits");
        drop(first);
        assert!(try_lock(&path).unwrap().is_some(), "free again once dropped");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A temp home with a `Grok Shell` holding a WAL database.
    fn home_with_grok_shell(name: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!("buwiz-broker-move-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let old = buwiz_protocol::broker_dir::old_dir(&home);
        std::fs::create_dir_all(old.join("generated")).unwrap();
        let db = rusqlite::Connection::open(old.join("generated/app.db")).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t(x); INSERT INTO t VALUES (7);").unwrap();
        home
    }

    /// Before its start the app moves the folder, after a backup, and
    /// launches the broker on the new one.
    #[test]
    fn the_start_moves_grok_shell_to_buwiz_broker_after_a_backup() {
        use buwiz_protocol::broker_dir::{new_dir, old_dir, Move, MOVED_MARKER};
        let home = home_with_grok_shell("start");
        assert_eq!(data_dir(&home), old_dir(&home), "read from the old one until it moves");
        let Move::Moved { backup: Some(backup), linked: true, .. } = move_data_dir(&home, &|| false) else { panic!("not moved") };
        let new = new_dir(&home);
        assert!(old_dir(&home).symlink_metadata().unwrap().file_type().is_symlink() && new.join(MOVED_MARKER).is_file());
        let copy = rusqlite::Connection::open(backup.join("app.db")).unwrap();
        assert_eq!(copy.query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0)).unwrap(), 7);
        let l = launch(Path::new("/A/Rox.app/Contents/Resources/broker/buwiz-broker"), &home, 8787);
        assert_eq!(l.data, new);
        assert!(l.args.contains(&new.clone().into_os_string()));
        assert_eq!(move_data_dir(&home, &|| false), Move::AlreadyThere, "once");
        let _ = std::fs::remove_dir_all(home);
    }

    /// A broker answering by then: no move, the old folder is launched on.
    #[test]
    fn a_broker_answering_at_the_last_probe_keeps_grok_shell() {
        use buwiz_protocol::broker_dir::{new_dir, old_dir, Move};
        let home = home_with_grok_shell("busy");
        assert_eq!(move_data_dir(&home, &|| true), Move::Busy);
        assert!(!new_dir(&home).exists());
        assert_eq!(launch(Path::new("/A/Rox.app/Contents/Resources/broker/buwiz-broker"), &home, 8787).data, old_dir(&home));
        let _ = std::fs::remove_dir_all(home);
    }
}
