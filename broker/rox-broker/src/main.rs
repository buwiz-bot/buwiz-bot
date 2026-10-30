//! rox-broker: binds the broker's port first (a second instance exits
//! without starting Node), then runs Node behind it and serves.

use std::sync::Arc;
use std::time::Duration;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rox_broker::config::Config;
use rox_broker::helper::line::Line;
use rox_broker::helper::mcp_servers::McpHost;
use rox_broker::helper::plugins::{Clock, Host, Options, Timeouts};
use rox_broker::hub::{ctx::HubCtx, Hub};
use rox_broker::leaf::Leaf;
use rox_broker::node::{self, Node};
use rox_broker::proxy::FrontDoor;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keystore-check") => std::process::exit(keystore_check(&args[1..])),
        Some("sign-manifest") => std::process::exit(match rox_broker::v1::signing::cli(&args[1..]) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("sign-manifest: {}", rox_broker::js::slice_utf16(&e, 300));
                1
            }
        }),
        Some("manifest") => std::process::exit(manifest_op(&args[1..])),
        Some("helper") => std::process::exit(helper(&args[1..]).await),
        _ => {}
    }
    let mut config = match Config::parse(&args, std::env::var("PORT").ok().as_deref()) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("rox-broker: {e}");
            std::process::exit(2);
        }
    };
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", config.port)).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("rox-broker: port {} is taken ({e}); not starting", config.port);
            std::process::exit(1);
        }
    };
    // what this broker hosts behind the line, for Node (ROX_LINE_HOSTS):
    // v1 plugins (phase 3) and v0 MCP servers (phase 4), each switched on
    let hosts: Vec<&str> = [("v1", "ROX_BROKER_V1_HOST"), ("mcp", "ROX_BROKER_MCP_HOST")]
        .into_iter()
        .filter(|(_, var)| std::env::var(var).as_deref() == Ok("1"))
        .map(|(name, _)| name)
        .collect();
    let line = if hosts.is_empty() {
        None
    } else {
        let data = config.data_dir(std::env::var("GROK_DATA_DIR").ok().as_deref());
        let trust = config.trust_file(std::env::var("ROX_TRUST_FILE").ok().as_deref());
        let db = data.join("generated").join("app.db");
        match (plugin_host(&db, Some(data.join("v1").join("logs")), trust, false), McpHost::new(&db, config.port)) {
            (Ok(host), Ok(mcp)) => {
                let token = rox_v1::keys::b64(&rox_v1::keys::random::<32>());
                config.node_env.push(("ROX_V1_LINE".to_owned(), format!("http://127.0.0.1:{}", config.port)));
                config.node_env.push(("ROX_V1_LINE_TOKEN".to_owned(), token.clone()));
                config.node_env.push(("ROX_LINE_HOSTS".to_owned(), hosts.join(",")));
                Some(Arc::new(Line::new(host, mcp, token)))
            }
            (Err(e), _) | (_, Err(e)) => {
                eprintln!("rox-broker: the line: {e}; Node hosts everything");
                None
            }
        }
    };
    let allowed = rox_broker::guard::allowed_origins(config.port, std::env::var("GROK_ALLOWED_ORIGINS").ok().as_deref());
    // the hub (plan phase 5, D30): the brain's routes answered here, and
    // Node in its office role, holding none of the brain's state
    let hub = (std::env::var("ROX_BROKER_HUB").as_deref() == Ok("1")).then(|| {
        config.node_env.push(("ROX_NODE_ROLE".to_owned(), "office".to_owned()));
        let mut ctx = HubCtx::new(config.data_dir(std::env::var("GROK_DATA_DIR").ok().as_deref()), Clock::system());
        if let Some(line) = &line {
            ctx.plugins = hosts.contains(&"v1").then(|| line.host().clone());
            ctx.mcp = hosts.contains(&"mcp").then(|| line.mcp().clone());
        }
        let hub = Arc::new(Hub::new(ctx, allowed.clone()));
        hub.start();
        hub
    });
    let node = Arc::new(Node::default());
    let _ = node.version.set(node::version_of(&config.node));
    let supervisor = tokio::spawn(node::supervise(config.clone(), node.clone()));
    let leaf = (std::env::var("ROX_BROKER_FORWARD_ALL").as_deref() != Ok("1"))
        .then(|| Leaf::new(config.data_dir(std::env::var("GROK_DATA_DIR").ok().as_deref()), allowed.clone()));
    let door = FrontDoor::new(node.clone(), leaf);
    let door = match &line {
        // the /mcp routes are Rust's when the MCP servers are (plan 4.3)
        Some(line) if hosts.contains(&"mcp") => door.with_line(line.clone()).with_mcp(rox_broker::leaf::mcp::McpRoutes::new(line.mcp().clone(), allowed.clone())),
        Some(line) => door.with_line(line.clone()),
        None => door,
    };
    let door = Arc::new(match hub {
        Some(hub) => door.with_hub(hub),
        None => door,
    });
    eprintln!("rox-broker on http://127.0.0.1:{}", config.port);

    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
    let mut int = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).expect("SIGINT handler");
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let door = door.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |req| {
                        let door = door.clone();
                        async move { door.handle(req).await }
                    });
                    let _ = http1::Builder::new().keep_alive(true).serve_connection(TokioIo::new(stream), service).await;
                });
            }
            _ = term.recv() => break,
            _ = int.recv() => break,
        }
    }
    drop(listener);
    if let Some(line) = &line {
        line.host().stop_all().await;
        line.mcp().stop_all().await;
    }
    // inside the 3 s a caller gives before SIGKILL (the broker's tests do)
    node.stop(Duration::from_millis(2500)).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), supervisor).await;
    eprintln!("rox-broker: stopped");
}

/// `rox-broker keystore-check <plugin id> [--data DIR]`: whether the
/// keystore (`ROX_KEYSTORE`, else the Keychain) holds the plugin's key, and
/// whether its public half is the one `v1_plugins` recorded. Prints the
/// public key only, never the seed.
fn keystore_check(args: &[String]) -> i32 {
    use rox_broker::helper::keystore::Keystore;
    let (Some(id), rest) = (args.first(), args.get(1..).unwrap_or(&[])) else {
        eprintln!("usage: rox-broker keystore-check <plugin id> [--data DIR]");
        return 2;
    };
    let data = match rest {
        [flag, dir] if flag == "--data" => Some(std::path::PathBuf::from(dir)),
        [] => std::env::var("GROK_DATA_DIR").ok().filter(|d| !d.is_empty()).map(std::path::PathBuf::from),
        _ => {
            eprintln!("usage: rox-broker keystore-check <plugin id> [--data DIR]");
            return 2;
        }
    };
    let store = match Keystore::from_spec(&std::env::var("ROX_KEYSTORE").unwrap_or_default()) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("rox-broker: {e}");
            return 2;
        }
    };
    let public = match store.get(id) {
        Ok(Some(seed)) => match rox_v1::keys::SessionKey::from_seed(&seed) {
            Ok(key) => key.public(),
            Err(e) => {
                println!("{id}: {} holds a key that is not a seed ({e})", store.kind());
                return 1;
            }
        },
        Ok(None) => {
            println!("{id}: no key in the {}", store.kind());
            return 1;
        }
        Err(e) => {
            eprintln!("rox-broker: {e}");
            return 2;
        }
    };
    let recorded = data.and_then(|d| {
        let db = rusqlite::Connection::open_with_flags(d.join("generated").join("app.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        db.query_row("SELECT plugin_pub FROM v1_plugins WHERE id=?", [id], |r| r.get::<_, String>(0)).ok()
    });
    let verdict = match recorded {
        Some(r) if r == public => "matches v1_plugins",
        Some(_) => "DIFFERS from v1_plugins",
        None => "not in v1_plugins",
    };
    println!("{id}: {} key found, public key {public} ({verdict})", store.kind());
    i32::from(verdict.starts_with("DIFFERS"))
}

/// `rox-broker manifest <op>`: one manifest check on the JSON request on
/// stdin, its answer on stdout (`v1::cli`).
fn manifest_op(args: &[String]) -> i32 {
    use std::io::Read as _;
    let mut input = String::new();
    let parsed = std::io::stdin().read_to_string(&mut input).map_err(|e| e.to_string()).and_then(|_| rox_v1::jcs::parse_ijson(&input));
    let answer = match (args, parsed) {
        ([op], Ok(input)) => rox_broker::v1::cli::run(op, &input),
        (_, Err(e)) => {
            eprintln!("rox-broker manifest: stdin is not JSON ({e})");
            return 2;
        }
        _ => None,
    };
    match answer {
        Some(answer) => {
            println!("{}", rox_broker::js::stringify(&answer));
            0
        }
        None => {
            eprintln!("usage: rox-broker manifest <verify|tools|props|review|added|semver|limits> < request.json");
            2
        }
    }
}

/// The v1 plugin host on `db`, its keystore from `ROX_KEYSTORE`.
fn plugin_host(db: &std::path::Path, log_dir: Option<std::path::PathBuf>, trust_file: std::path::PathBuf, manual_clock: bool) -> Result<Arc<Host>, String> {
    let keys = rox_broker::helper::keystore::Keystore::from_spec(&std::env::var("ROX_KEYSTORE").unwrap_or_default())?;
    let mut timeouts = Timeouts::default();
    // ROX_V1_CALL_TIMEOUT_MS shortens the 120 s call limit (tests)
    if let Some(ms) = std::env::var("ROX_V1_CALL_TIMEOUT_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|ms| *ms > 0) {
        timeouts.call = Duration::from_millis(ms);
    }
    let clock = if manual_clock { Clock::manual() } else { Clock::system() };
    Host::new(Options { db: db.to_path_buf(), keys, trust_file, log_dir, clock, timeouts })
}

/// `rox-broker helper --db FILE [--log-dir DIR] [--port P]`: the v1
/// plugin host alone, its line on 127.0.0.1 (port 0 picks one; the first
/// line of stdout names it). `ROX_LINE_TOKEN` is the line's token,
/// `ROX_TRUST_FILE` the trust store, `ROX_V1_MANUAL_CLOCK=1` a clock the
/// line moves (tests).
async fn helper(args: &[String]) -> i32 {
    let usage = "usage: rox-broker helper --db FILE [--log-dir DIR] [--port P]";
    let (mut db, mut log_dir, mut port) = (None, None, 0u16);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--db", Some(v)) => db = Some(std::path::PathBuf::from(v)),
            ("--log-dir", Some(v)) => log_dir = Some(std::path::PathBuf::from(v)),
            ("--port", Some(v)) if v.parse::<u16>().is_ok() => port = v.parse().unwrap_or(0),
            _ => {
                eprintln!("{usage}");
                return 2;
            }
        }
    }
    let (Some(db), Ok(token)) = (db, std::env::var("ROX_LINE_TOKEN")) else {
        eprintln!("{usage} (with ROX_LINE_TOKEN set)");
        return 2;
    };
    let trust = std::env::var("ROX_TRUST_FILE").map(std::path::PathBuf::from).unwrap_or_else(|_| "src/v1/trust.json".into());
    let host = match plugin_host(&db, log_dir, trust, std::env::var("ROX_V1_MANUAL_CLOCK").as_deref() == Ok("1")) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("rox-broker helper: {e}");
            return 1;
        }
    };
    // the redirect OAuth flows name: the front door's (Node's PUBLIC_PORT)
    let public = ["ROX_PUBLIC_PORT", "PORT"].iter().find_map(|k| std::env::var(k).ok()?.parse::<u16>().ok()).unwrap_or(8787);
    let mcp = match McpHost::new(&db, public) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("rox-broker helper: {e}");
            return 1;
        }
    };
    let line = Arc::new(Line::new(host.clone(), mcp.clone(), token));
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("rox-broker helper: port {port}: {e}");
            return 1;
        }
    };
    println!("rox-broker helper on 127.0.0.1:{}", listener.local_addr().map(|a| a.port()).unwrap_or(port));
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
    let mut int = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).expect("SIGINT handler");
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let line = line.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                        let line = line.clone();
                        async move { Ok::<_, std::convert::Infallible>(line.serve(req).await) }
                    });
                    let _ = http1::Builder::new().keep_alive(true).serve_connection(TokioIo::new(stream), service).await;
                });
            }
            _ = term.recv() => break,
            _ = int.recv() => break,
        }
    }
    host.stop_all().await;
    mcp.stop_all().await;
    0
}
