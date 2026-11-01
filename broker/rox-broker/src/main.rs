//! rox-broker: binds the broker's port first (a second instance exits
//! without starting Node), then runs Node behind it and serves. With the
//! hub on and `BUWIZ_BROKER_NO_NODE=1` it starts no Node at all: every route
//! is Rust's or the hub's 404 (plan phase 7).

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
        Some("office-mcp") => std::process::exit(rox_broker::office::mcp::cli(&args[1..])),
        _ => {}
    }
    let mut config = match Config::parse(&args, std::env::var("PORT").ok().as_deref()) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("rox-broker: {e}");
            std::process::exit(2);
        }
    };
    let hub_on = rox_v1::env::var("BUWIZ_BROKER_HUB").as_deref() == Some("1");
    // `--no-node` (the broker inside Rox.app, 7.1) or BUWIZ_BROKER_NO_NODE=1
    let no_node = config.no_node || rox_v1::env::var("BUWIZ_BROKER_NO_NODE").as_deref() == Some("1");
    if no_node && !hub_on {
        eprintln!("rox-broker: --no-node (BUWIZ_BROKER_NO_NODE=1) needs BUWIZ_BROKER_HUB=1 (the hub answers what Node would)");
        std::process::exit(2);
    }
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", config.port)).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("rox-broker: port {} is taken ({e}); not starting", config.port);
            std::process::exit(1);
        }
    };
    // what this broker hosts behind the line, for Node (BUWIZ_LINE_HOSTS):
    // v1 plugins (phase 3) and v0 MCP servers (phase 4), each switched on
    let hosts: Vec<&str> = [("v1", "BUWIZ_BROKER_V1_HOST"), ("mcp", "BUWIZ_BROKER_MCP_HOST")]
        .into_iter()
        .filter(|(_, var)| rox_v1::env::var(var).as_deref() == Some("1"))
        .map(|(name, _)| name)
        .collect();
    let line = if hosts.is_empty() {
        None
    } else {
        let data = config.data_dir(rox_v1::env::var("BUWIZ_BROKER_DATA_DIR").as_deref());
        let trust = config.trust_file(std::env::var("BUWIZ_TRUST_FILE").ok().as_deref());
        let db = data.join("generated").join("app.db");
        match (plugin_host(&db, Some(data.join("v1").join("logs")), trust, false, Some(data.join("v1").join("apps")), config.port), McpHost::new(&db, config.port)) {
            (Ok(host), Ok(mcp)) => {
                let token = rox_v1::keys::b64(&rox_v1::keys::random::<32>());
                config.node_env.push(("BUWIZ_V1_LINE".to_owned(), format!("http://127.0.0.1:{}", config.port)));
                config.node_env.push(("BUWIZ_V1_LINE_TOKEN".to_owned(), token.clone()));
                config.node_env.push(("BUWIZ_LINE_HOSTS".to_owned(), hosts.join(",")));
                Some(Arc::new(Line::new(host, mcp, token)))
            }
            (Err(e), _) | (_, Err(e)) => {
                eprintln!("rox-broker: the line: {e}; Node hosts everything");
                None
            }
        }
    };
    let allowed = rox_broker::guard::allowed_origins(config.port, rox_v1::env::var("BUWIZ_ALLOWED_ORIGINS").as_deref());
    // the hub (plan phase 5, D30): the brain's routes answered here, and
    // Node in its office role, holding none of the brain's state; so this
    // broker hosts the v1 plugins and MCP servers the brain calls, the
    // line's when it has them, else its own
    let hub = hub_on.then(|| {
        config.node_env.push(("BUWIZ_NODE_ROLE".to_owned(), "office".to_owned()));
        let data = config.data_dir(rox_v1::env::var("BUWIZ_BROKER_DATA_DIR").as_deref());
        let db = data.join("generated").join("app.db");
        let mut ctx = HubCtx::new(data.clone(), Clock::system());
        if let Some(line) = &line {
            if hosts.contains(&"v1") {
                ctx.plugins = Some(line.host().clone());
                ctx.sessions = Some(line.sessions().clone());
            }
            ctx.mcp = hosts.contains(&"mcp").then(|| line.mcp().clone());
        }
        if ctx.plugins.is_none() {
            match plugin_host(&db, Some(data.join("v1").join("logs")), config.trust_file(std::env::var("BUWIZ_TRUST_FILE").ok().as_deref()), false, Some(data.join("v1").join("apps")), config.port) {
                Ok(host) => {
                    ctx.sessions = Some(rox_broker::helper::sessions::Sessions::new(host.clone()));
                    ctx.plugins = Some(host);
                }
                Err(e) => eprintln!("rox-broker: the hub's plugin host: {e}"),
            }
        }
        if ctx.mcp.is_none() {
            match McpHost::new(&db, config.port) {
                Ok(mcp) => ctx.mcp = Some(mcp),
                Err(e) => eprintln!("rox-broker: the hub's MCP host: {e}"),
            }
        }
        let hub = Arc::new(Hub::new(ctx, allowed.clone()));
        hub.start();
        hub
    });
    // no Node: nothing started, not even `node --version` for /meta
    let node = Arc::new(if no_node { Node::absent() } else { Node::default() });
    let _ = node.version.set(if no_node { String::new() } else { node::version_of(&config.node) });
    let supervisor = (!no_node).then(|| tokio::spawn(node::supervise(config.clone(), node.clone())));
    let leaf = (rox_v1::env::var("BUWIZ_BROKER_FORWARD_ALL").as_deref() != Some("1"))
        .then(|| Leaf::new(config.data_dir(rox_v1::env::var("BUWIZ_BROKER_DATA_DIR").as_deref()), allowed.clone()));
    let door = FrontDoor::new(node.clone(), leaf);
    let door = match &line {
        // the /mcp routes are Rust's when the MCP servers are (plan 4.3)
        Some(line) if hosts.contains(&"mcp") => {
            let routes = rox_broker::leaf::mcp::McpRoutes::new(line.mcp().clone(), allowed.clone());
            // a mini app account's sign-in comes back on the same callback route
            let routes = if hosts.contains(&"v1") { routes.with_plugins(line.host().clone()) } else { routes };
            door.with_line(line.clone()).with_mcp(routes)
        }
        Some(line) => door.with_line(line.clone()),
        None => door,
    };
    let hub_ctx = hub.as_ref().map(|h| h.ctx().clone());
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
    if let Some(ctx) = &hub_ctx {
        if let Some(host) = &ctx.plugins {
            host.stop_all().await;
        }
        if let Some(mcp) = &ctx.mcp {
            mcp.stop_all().await;
        }
    }
    // inside the 3 s a caller gives before SIGKILL (the broker's tests do)
    node.stop(Duration::from_millis(2500)).await;
    if let Some(supervisor) = supervisor {
        let _ = tokio::time::timeout(Duration::from_secs(2), supervisor).await;
    }
    eprintln!("rox-broker: stopped");
}

/// `rox-broker keystore-check <plugin id> [--data DIR]`: whether the
/// keystore (`BUWIZ_KEYSTORE`, else the Keychain) holds the plugin's key, and
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
        [] => rox_v1::env::var("BUWIZ_BROKER_DATA_DIR").filter(|d| !d.is_empty()).map(std::path::PathBuf::from),
        _ => {
            eprintln!("usage: rox-broker keystore-check <plugin id> [--data DIR]");
            return 2;
        }
    };
    let store = match Keystore::from_spec(&std::env::var("BUWIZ_KEYSTORE").unwrap_or_default()) {
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
            eprintln!("usage: rox-broker manifest <verify|tools|props|review|added|semver|limits|package> < request.json");
            2
        }
    }
}

/// The v1 plugin host on `db`, its keystore from `BUWIZ_KEYSTORE`; `port`
/// the front door's (account sign-ins redirect there).
fn plugin_host(db: &std::path::Path, log_dir: Option<std::path::PathBuf>, trust_file: std::path::PathBuf, manual_clock: bool, apps_dir: Option<std::path::PathBuf>, port: u16) -> Result<Arc<Host>, String> {
    let keys = rox_broker::helper::keystore::Keystore::from_spec(&std::env::var("BUWIZ_KEYSTORE").unwrap_or_default())?;
    let mut timeouts = Timeouts::default();
    // BUWIZ_V1_CALL_TIMEOUT_MS shortens the 120 s call limit (tests)
    if let Some(ms) = rox_v1::env::var("BUWIZ_V1_CALL_TIMEOUT_MS").and_then(|v| v.parse::<u64>().ok()).filter(|ms| *ms > 0) {
        timeouts.call = Duration::from_millis(ms);
    }
    let clock = if manual_clock { Clock::manual() } else { Clock::system() };
    let host = Host::new(Options { db: db.to_path_buf(), keys: keys.clone(), trust_file, log_dir, clock, timeouts, apps_dir })?;
    host.set_accounts_config(rox_broker::helper::plugins::AccountsConfig { port, ..Default::default() });
    // Rox's plugin keys, under the new Keychain service before any plugin starts
    let ids: Vec<String> = host.rows().into_iter().map(|r| r.id).collect();
    match keys.move_from_rox(&ids, &db.with_file_name(rox_broker::helper::keystore::MOVED_MARKER)) {
        Ok(0) => {}
        Ok(n) => eprintln!("  keystore: {n} plugin key(s) moved from {}", rox_broker::helper::keystore::OLD_KEYCHAIN_SERVICE),
        Err(e) => eprintln!("  keystore: plugin keys not moved from {} yet: {e}", rox_broker::helper::keystore::OLD_KEYCHAIN_SERVICE),
    }
    Ok(host)
}

/// `rox-broker helper --db FILE [--log-dir DIR] [--port P]`: the v1
/// plugin host alone, its line on 127.0.0.1 (port 0 picks one; the first
/// line of stdout names it). `BUWIZ_LINE_TOKEN` is the line's token,
/// `BUWIZ_TRUST_FILE` the trust store, `BUWIZ_V1_MANUAL_CLOCK=1` a clock the
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
    let (Some(db), Some(token)) = (db, rox_v1::env::var("BUWIZ_LINE_TOKEN")) else {
        eprintln!("{usage} (with BUWIZ_LINE_TOKEN set)");
        return 2;
    };
    let trust = std::env::var("BUWIZ_TRUST_FILE").map(std::path::PathBuf::from).unwrap_or_else(|_| "src/v1/trust.json".into());
    // the redirect OAuth flows name: the front door's (Node's PUBLIC_PORT)
    let public = ["BUWIZ_PUBLIC_PORT", "PORT"].iter().find_map(|k| rox_v1::env::var(k)?.parse::<u16>().ok()).unwrap_or(8787);
    let host = match plugin_host(&db, log_dir, trust, rox_v1::env::var("BUWIZ_V1_MANUAL_CLOCK").as_deref() == Some("1"), None, public) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("rox-broker helper: {e}");
            return 1;
        }
    };
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
