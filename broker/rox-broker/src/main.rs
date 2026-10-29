//! rox-broker: binds the broker's port first (a second instance exits
//! without starting Node), then runs Node behind it and serves.

use std::sync::Arc;
use std::time::Duration;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rox_broker::config::Config;
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
        _ => {}
    }
    let config = match Config::parse(&args, std::env::var("PORT").ok().as_deref()) {
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
    let node = Arc::new(Node::default());
    let _ = node.version.set(node::version_of(&config.node));
    let supervisor = tokio::spawn(node::supervise(config.clone(), node.clone()));
    let allowed = rox_broker::guard::allowed_origins(config.port, std::env::var("GROK_ALLOWED_ORIGINS").ok().as_deref());
    let leaf = (std::env::var("ROX_BROKER_FORWARD_ALL").as_deref() != Ok("1"))
        .then(|| Leaf::new(config.data_dir(std::env::var("GROK_DATA_DIR").ok().as_deref()), allowed));
    let door = Arc::new(FrontDoor::new(node.clone(), leaf));
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
