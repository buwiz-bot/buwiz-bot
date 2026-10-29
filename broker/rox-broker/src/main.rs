//! rox-broker: binds the broker's port first (a second instance exits
//! without starting Node), then runs Node behind it and serves.

use std::sync::Arc;
use std::time::Duration;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rox_broker::config::Config;
use rox_broker::node::{self, Node};
use rox_broker::proxy::FrontDoor;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
    let door = Arc::new(FrontDoor::new(node.clone()));
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
    node.stop(Duration::from_secs(5)).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), supervisor).await;
    eprintln!("rox-broker: stopped");
}
