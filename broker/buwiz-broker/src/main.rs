//! buwiz-broker: binds the broker's port first (a second instance exits),
//! then serves every route itself (`front_door`).

use std::sync::Arc;
use std::time::Duration;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use buwiz_broker::config::Config;
use buwiz_broker::helper::mcp_servers::McpHost;
use buwiz_broker::helper::plugins::{Clock, Host, Options, Timeouts};
use buwiz_broker::hub::{ctx::HubCtx, Hub};
use buwiz_broker::leaf::Leaf;
use buwiz_broker::front_door::FrontDoor;

#[tokio::main]
async fn main() {
    if let Err(why) = buwiz_protocol::env::refuse_old_names() {
        eprintln!("buwiz-broker: {why}");
        std::process::exit(2);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keystore-check") => std::process::exit(keystore_check(&args[1..])),
        Some("sign-manifest") => std::process::exit(match buwiz_broker::v1::signing::cli(&args[1..]) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("sign-manifest: {}", buwiz_broker::js::slice_utf16(&e, 300));
                1
            }
        }),
        Some("manifest") => std::process::exit(manifest_op(&args[1..])),
        Some("office-mcp") => std::process::exit(buwiz_broker::office::mcp::cli(&args[1..])),
        _ => {}
    }
    let mut config = match Config::parse(&args, std::env::var("PORT").ok().as_deref()) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("buwiz-broker: {e}");
            std::process::exit(2);
        }
    };
    buwiz_broker::hub::market::init_paths(&config);
    // read once: a setting this build can't use stops it here, and nothing
    // downstream carries on without one or falls back to the real Google
    let accounts = match buwiz_broker::helper::plugins::AccountsConfig::from_env(config.port) {
        Ok(accounts) => accounts,
        Err(e) => {
            eprintln!("buwiz-broker: {e}");
            std::process::exit(2);
        }
    };
    // read once: the plugin trust list the broker's plugin hosts use
    let trust_env = std::env::var("BUWIZ_TRUST_FILE").ok();
    let trust = config.trust_file(trust_env.as_deref());
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", config.port)).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("buwiz-broker: port {} is taken ({e}); not starting", config.port);
            std::process::exit(1);
        }
    };
    // the owner's folder: Grok Shell moves to Buwiz Broker once, before
    // anything opens it (buwiz_protocol::broker_dir). This broker holds its port
    // now; on another port it checks that none answers on the owner's.
    let home = home();
    let own_port = config.port;
    let busy = move || own_port != buwiz_protocol::broker_dir::OWNER_PORT && buwiz_protocol::broker_dir::port_answers(buwiz_protocol::broker_dir::OWNER_PORT);
    match config.settle_owner_dir(buwiz_protocol::env::var("BUWIZ_BROKER_DATA_DIR").as_deref(), &home, &busy) {
        None | Some(buwiz_protocol::broker_dir::Move::Nothing | buwiz_protocol::broker_dir::Move::AlreadyThere) => {}
        Some(buwiz_protocol::broker_dir::Move::Moved { backup, rewritten, linked }) => {
            eprintln!(
                "buwiz-broker: moved {} to {} (database backup: {}; the old name left as a link: {linked})",
                buwiz_protocol::broker_dir::OLD_DIR_NAME,
                buwiz_protocol::broker_dir::DIR_NAME,
                backup.map(|b| b.display().to_string()).unwrap_or_else(|| "none, no database".into())
            );
            for (column, n) in rewritten {
                eprintln!("  {column}: {n} stored path(s) rewritten");
            }
        }
        Some(m) => eprintln!("buwiz-broker: {} not moved to {} this time ({m:?}); using it", buwiz_protocol::broker_dir::OLD_DIR_NAME, buwiz_protocol::broker_dir::DIR_NAME),
    }
    let allowed = buwiz_broker::guard::allowed_origins(config.port, buwiz_protocol::env::var("BUWIZ_ALLOWED_ORIGINS").as_deref());
    // The hub is always on. BUWIZ_BROKER_HUB, BUWIZ_BROKER_V1_HOST and
    // BUWIZ_BROKER_MCP_HOST (and `--no-node`) once chose between this broker
    // a part of it; they are accepted, never read, and do nothing.
    // The hub hosts the v1 plugins and the MCP servers its routes call.
    let data = config.data_dir(buwiz_protocol::env::var("BUWIZ_BROKER_DATA_DIR").as_deref());
    let db = data.join("generated").join("app.db");
    let mut ctx = HubCtx::new(data.clone(), Clock::system());
    match plugin_host(&data, &db, Some(data.join("v1").join("logs")), trust, Some(data.join("v1").join("apps")), accounts) {
        Ok(host) => {
            ctx.sessions = Some(buwiz_broker::helper::sessions::Sessions::new(host.clone()));
            ctx.plugins = Some(host);
        }
        Err(e) => eprintln!("buwiz-broker: the hub's plugin host: {e}"),
    }
    match McpHost::new(&db, config.port) {
        Ok(mcp) => ctx.mcp = Some(mcp),
        Err(e) => eprintln!("buwiz-broker: the hub's MCP host: {e}"),
    }
    let hub = Arc::new(Hub::new(ctx, allowed.clone()));
    hub.start();
    let hub_ctx = hub.ctx().clone();
    let door = FrontDoor::new(Leaf::new(data, allowed.clone()), hub);
    // the /mcp routes, answered for the Bot of the chat they are for
    let door = match hub_ctx.mcp.clone() {
        Some(mcp) => {
            let routes = buwiz_broker::leaf::mcp::McpRoutes::new(mcp, allowed.clone(), hub_ctx.clone());
            // a mini app account's sign-in comes back on the same callback route
            let routes = match hub_ctx.plugins.clone() {
                Some(host) => routes.with_plugins(host),
                None => routes,
            };
            door.with_mcp(routes)
        }
        None => door,
    };
    // a mini app account's sign-in callback reaches the plugin host, MCP host or not
    let door = Arc::new(match hub_ctx.plugins.clone() {
        Some(host) => door.with_accounts(host),
        None => door,
    });
    eprintln!("buwiz-broker on http://127.0.0.1:{}", config.port);
    let off = config.off_without_assets(trust_env.as_deref());
    if !off.is_empty() {
        eprintln!("buwiz-broker: started without --assets, so off: {}", off.join(", "));
    } else if config.assets_lack_a_trust_list(trust_env.as_deref()) {
        eprintln!("buwiz-broker: --assets has no trust.json and BUWIZ_TRUST_FILE is not set, so off: the plugin trust list");
    }

    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
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
            _ = &mut shutdown => break,
        }
    }
    drop(listener);
    if let Some(computers) = buwiz_broker::hub::computer::Computers::of(&hub_ctx) {
        computers.let_the_mac_sleep();
    }
    if let Some(host) = &hub_ctx.plugins {
        host.stop_all().await;
    }
    if let Some(mcp) = &hub_ctx.mcp {
        mcp.stop_all().await;
    }
    eprintln!("buwiz-broker: stopped");
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
        let mut int = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).expect("SIGINT handler");
        tokio::select! {
            _ = term.recv() => {},
            _ = int.recv() => {},
        }
    }
    #[cfg(windows)]
    tokio::signal::ctrl_c().await.expect("Ctrl-C handler");
}

/// `buwiz-broker keystore-check <plugin id> [--data DIR]`: whether the
/// keystore (`BUWIZ_KEYSTORE`, else the one of the data dir) holds the plugin's key, and
/// whether its public half is the one `v1_plugins` recorded. Prints the
/// public key only, never the seed.
fn keystore_check(args: &[String]) -> i32 {
    let (Some(id), rest) = (args.first(), args.get(1..).unwrap_or(&[])) else {
        eprintln!("usage: buwiz-broker keystore-check <plugin id> [--data DIR]");
        return 2;
    };
    let data = match rest {
        [flag, dir] if flag == "--data" => Some(std::path::PathBuf::from(dir)),
        [] => buwiz_protocol::env::var("BUWIZ_BROKER_DATA_DIR").filter(|d| !d.is_empty()).map(std::path::PathBuf::from),
        _ => {
            eprintln!("usage: buwiz-broker keystore-check <plugin id> [--data DIR]");
            return 2;
        }
    };
    let store = match data_keystore(data.as_deref().unwrap_or(&buwiz_protocol::broker_dir::dir_in(&home()))) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("buwiz-broker: {e}");
            return 2;
        }
    };
    let public = match store.get(id) {
        Ok(Some(seed)) => match buwiz_protocol::keys::SessionKey::from_seed(&seed) {
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
            eprintln!("buwiz-broker: {e}");
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

/// `buwiz-broker manifest <op>`: one manifest check on the JSON request on
/// stdin, its answer on stdout (`v1::cli`).
fn manifest_op(args: &[String]) -> i32 {
    use std::io::Read as _;
    let mut input = String::new();
    let parsed = std::io::stdin().read_to_string(&mut input).map_err(|e| e.to_string()).and_then(|_| buwiz_protocol::jcs::parse_ijson(&input));
    let answer = match (args, parsed) {
        ([op], Ok(input)) => buwiz_broker::v1::cli::run(op, &input),
        (_, Err(e)) => {
            eprintln!("buwiz-broker manifest: stdin is not JSON ({e})");
            return 2;
        }
        _ => None,
    };
    match answer {
        Some(answer) => {
            println!("{}", buwiz_broker::js::stringify(&answer));
            0
        }
        None => {
            eprintln!("usage: buwiz-broker manifest <verify|tools|props|review|added|semver|limits|package> < request.json");
            2
        }
    }
}

fn home() -> std::path::PathBuf {
    #[cfg(unix)]
    { std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default() }
    #[cfg(windows)]
    { std::env::home_dir().unwrap_or_default() }
}

/// The keystore of a broker on `data` ([`Keystore::for_data`]).
///
/// [`Keystore::for_data`]: buwiz_broker::helper::keystore::Keystore::for_data
fn data_keystore(data: &std::path::Path) -> Result<buwiz_broker::helper::keystore::Keystore, String> {
    let no_keychain = std::env::var("BUWIZ_NO_KEYCHAIN").as_deref() == Ok("1");
    let spec = std::env::var("BUWIZ_KEYSTORE").unwrap_or_default();
    buwiz_broker::helper::keystore::Keystore::for_data(&spec, no_keychain, data, &home())
}

/// The v1 plugin host on `db`, its keystore that of data dir `data`;
/// `accounts` where its sign-ins go.
fn plugin_host(data: &std::path::Path, db: &std::path::Path, log_dir: Option<std::path::PathBuf>, trust_file: Option<std::path::PathBuf>, apps_dir: Option<std::path::PathBuf>, accounts: buwiz_broker::helper::plugins::AccountsConfig) -> Result<Arc<Host>, String> {
    let keys = data_keystore(data)?;
    let mut timeouts = Timeouts::default();
    // BUWIZ_V1_CALL_TIMEOUT_MS shortens the 120 s call limit (tests)
    if let Some(ms) = buwiz_protocol::env::var("BUWIZ_V1_CALL_TIMEOUT_MS").and_then(|v| v.parse::<u64>().ok()).filter(|ms| *ms > 0) {
        timeouts.call = Duration::from_millis(ms);
    }
    let host = Host::new(Options { db: db.to_path_buf(), keys: keys.clone(), trust_file, log_dir, clock: Clock::system(), timeouts, apps_dir })?;
    host.set_accounts_config(accounts);
    // Buwiz Bot's plugin keys, under the new Keychain service before any plugin starts
    let ids: Vec<String> = host.rows().into_iter().map(|r| r.id).collect();
    match keys.move_from_rox(&ids, &db.with_file_name(buwiz_broker::helper::keystore::MOVED_MARKER)) {
        Ok(0) => {}
        Ok(n) => eprintln!("  keystore: {n} plugin key(s) moved from {}", buwiz_broker::helper::keystore::OLD_KEYCHAIN_SERVICE),
        Err(e) => eprintln!("  keystore: plugin keys not moved from {} yet: {e}", buwiz_broker::helper::keystore::OLD_KEYCHAIN_SERVICE),
    }
    Ok(host)
}
