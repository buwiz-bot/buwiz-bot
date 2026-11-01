//! The command line and environment.
//!
//!   buwiz-broker [--port P] [--data DIR] [--node PATH] [--node-script PATH]
//!              [--assets DIR] [--no-node]
//!
//! The port comes from `--port`, else `PORT`, else 8787. Node runs
//! `--node-script` (default `src/app-server.mjs`, from the working
//! directory, as the broker's tests run it) with `--node` (default `node`).
//! `--no-node` runs no Node at all (the hub answers everything; plan 7.1).
//! `--assets` is the folder of the static files the broker serves and
//! reads (office pages, the plugin catalog, trust.json; `node_modules`
//! beside it): Node's server dir, the script's folder, when not given, as
//! a checkout and the Grok Shell bundle lay them out (plan 7.1).
//! `--data` sets `BUWIZ_BROKER_DATA_DIR` for Node; without it Node keeps the
//! environment's. The routes answered in Rust use the same data dir as
//! Node (`data_dir`).
//!
//! Given the owner's folder (`~/Library/Application Support/Buwiz Broker`,
//! or `Grok Shell`, its old name), the broker moves `Grok Shell` to `Buwiz
//! Broker` once before it opens anything there ([`Config::settle_owner_dir`],
//! `buwiz_protocol::broker_dir`), and uses the folder that holds the data. Any other
//! data dir (a test's, a bench's, a checkout's) is used as given.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub port: u16,
    pub data: Option<PathBuf>,
    pub node: PathBuf,
    pub node_script: PathBuf,
    /// `--assets`: the server dir, when it is not the script's folder.
    pub assets: Option<PathBuf>,
    /// `--no-node`: no Node behind the front door.
    pub no_node: bool,
    /// More environment for Node (the line's address and token).
    pub node_env: Vec<(String, String)>,
}

impl Config {
    pub fn parse(args: &[String], env_port: Option<&str>) -> Result<Self, String> {
        let mut port = env_port.map(str::to_owned);
        let mut data = None;
        let mut node = PathBuf::from("node");
        let mut node_script = PathBuf::from("src/app-server.mjs");
        let (mut assets, mut no_node) = (None, false);
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let mut value = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
            match arg.as_str() {
                "--port" => port = Some(value("--port")?),
                "--data" => data = Some(PathBuf::from(value("--data")?)),
                "--node" => node = PathBuf::from(value("--node")?),
                "--node-script" => node_script = PathBuf::from(value("--node-script")?),
                "--assets" => assets = Some(PathBuf::from(value("--assets")?)),
                "--no-node" => no_node = true,
                other => return Err(format!("unknown argument {other}")),
            }
        }
        let port = match port.filter(|p| !p.is_empty()) {
            Some(p) => p.parse().map_err(|_| format!("the port {p} is not a number"))?,
            None => 8787,
        };
        Ok(Self { port, data, node, node_script, assets, no_node, node_env: Vec::new() })
    }

    /// Node's server dir (its `serverFile()` root): `--assets`, else the
    /// script's folder (`.` for a bare script name).
    pub fn server_dir(&self) -> PathBuf {
        if let Some(dir) = &self.assets {
            return dir.clone();
        }
        self.node_script.parent().filter(|p| !p.as_os_str().is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
    }

    /// The trust store Node reads (v1/trust.mjs): `BUWIZ_TRUST_FILE`, else
    /// `trust.json` in the server dir (a bundle), else `v1/trust.json`
    /// (a checkout).
    pub fn trust_file(&self, env: Option<&str>) -> PathBuf {
        if let Some(f) = env.filter(|e| !e.is_empty()) {
            return PathBuf::from(f);
        }
        let dir = self.server_dir();
        let beside = dir.join("trust.json");
        if beside.exists() {
            beside
        } else {
            dir.join("v1").join("trust.json")
        }
    }

    /// Node's data dir: `--data`, else `BUWIZ_BROKER_DATA_DIR`, else the folder
    /// above the script's (config.mjs's APP_ROOT).
    pub fn data_dir(&self, env: Option<&str>) -> PathBuf {
        if let Some(data) = &self.data {
            return data.clone();
        }
        if let Some(env) = env.filter(|e| !e.is_empty()) {
            return PathBuf::from(env);
        }
        match self.node_script.parent().and_then(|p| p.parent()) {
            Some(root) if !root.as_os_str().is_empty() => root.to_path_buf(),
            _ => PathBuf::from("."),
        }
    }
}

impl Config {
    /// When the data dir (`--data`, else `env_data`) is the owner's folder
    /// under `home`, by either name: moves `Grok Shell` to `Buwiz Broker`
    /// once (`busy`: a broker answers on the owner's port) and points `data` at the folder holding the
    /// data, and Node's `NSH_CHUNK_DIR` (`chunk_env`) at its `chunks` when it
    /// was under the old one. `None` for any other data dir: nothing is
    /// looked at.
    pub fn settle_owner_dir(
        &mut self,
        env_data: Option<&str>,
        chunk_env: Option<&str>,
        home: &std::path::Path,
        busy: &dyn Fn() -> bool,
    ) -> Option<buwiz_protocol::broker_dir::Move> {
        use buwiz_protocol::broker_dir;
        let given = self.data_dir(env_data);
        if !broker_dir::is_owners(home, &given) {
            return None;
        }
        let moved = broker_dir::move_once(home, busy);
        let dir = broker_dir::dir_in(home);
        if let Some(rest) = chunk_env.and_then(|c| std::path::Path::new(c).strip_prefix(&given).ok()) {
            if dir != given {
                self.node_env.push(("NSH_CHUNK_DIR".to_owned(), dir.join(rest).to_string_lossy().into_owned()));
            }
        }
        self.data = Some(dir);
        Some(moved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_port_comes_from_the_flag_then_the_environment_then_8787() {
        assert_eq!(Config::parse(&args(&[]), None).unwrap().port, 8787);
        assert_eq!(Config::parse(&args(&[]), Some("8798")).unwrap().port, 8798);
        assert_eq!(Config::parse(&args(&["--port", "8799"]), Some("8798")).unwrap().port, 8799);
        assert!(Config::parse(&args(&["--port", "x"]), None).is_err());
        assert!(Config::parse(&args(&["--nope"]), None).is_err());
    }

    #[test]
    fn the_data_dir_is_nodes() {
        let c = Config::parse(&args(&[]), None).unwrap();
        assert_eq!(c.data_dir(None), PathBuf::from("."));
        assert_eq!(c.data_dir(Some("")), PathBuf::from("."));
        assert_eq!(c.data_dir(Some("/env")), PathBuf::from("/env"));
        let c = Config::parse(&args(&["--node-script", "/r/server/app-server.mjs"]), None).unwrap();
        assert_eq!(c.data_dir(None), PathBuf::from("/r"));
        let c = Config::parse(&args(&["--data", "/d"]), None).unwrap();
        assert_eq!(c.data_dir(Some("/env")), PathBuf::from("/d"));
    }

    #[test]
    fn node_defaults_to_the_broker_script_beside_the_tests() {
        let c = Config::parse(&args(&["--data", "/d", "--node", "/n/node", "--node-script", "/n/app-server.mjs"]), None).unwrap();
        assert_eq!(c.data, Some(PathBuf::from("/d")));
        assert_eq!(c.node, PathBuf::from("/n/node"));
        assert_eq!(c.node_script, PathBuf::from("/n/app-server.mjs"));
        let d = Config::parse(&args(&[]), None).unwrap();
        assert_eq!((d.node, d.node_script), (PathBuf::from("node"), PathBuf::from("src/app-server.mjs")));
    }

    #[test]
    fn the_assets_default_to_the_scripts_folder_and_take_the_flag() {
        let d = Config::parse(&args(&[]), None).unwrap();
        assert_eq!((d.assets.clone(), d.no_node), (None, false));
        assert_eq!(d.server_dir(), PathBuf::from("src"));
        let c = Config::parse(&args(&["--node-script", "/r/server/app-server.mjs"]), None).unwrap();
        assert_eq!(c.server_dir(), PathBuf::from("/r/server"));
        assert_eq!(Config::parse(&args(&["--node-script", "app-server.mjs"]), None).unwrap().server_dir(), PathBuf::from("."));
        let c = Config::parse(&args(&["--node-script", "/r/server/app-server.mjs", "--assets", "/b/broker/server", "--no-node"]), None).unwrap();
        assert_eq!(c.assets, Some(PathBuf::from("/b/broker/server")));
        assert!(c.no_node);
        assert_eq!(c.server_dir(), PathBuf::from("/b/broker/server"));
        // the trust store is the assets' (no file there: the checkout's path under it)
        assert_eq!(c.trust_file(None), PathBuf::from("/b/broker/server/v1/trust.json"));
        assert!(Config::parse(&args(&["--assets"]), None).is_err());
    }

    /// A temp home, removed when dropped.
    struct Home(PathBuf);
    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn home(name: &str) -> Home {
        let dir = std::env::temp_dir().join(format!("buwiz-broker-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Library/Application Support")).unwrap();
        Home(dir)
    }
    fn grok_shell(h: &Home) -> PathBuf {
        let old = buwiz_protocol::broker_dir::old_dir(&h.0);
        std::fs::create_dir_all(old.join("generated")).unwrap();
        let db = rusqlite::Connection::open(old.join("generated/app.db")).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t(x); INSERT INTO t VALUES (42);").unwrap();
        old
    }

    /// Given the owner's old folder, the broker backs its database up,
    /// moves it and runs on the new one; Node's chunks follow.
    #[test]
    fn the_owners_folder_moves_once_with_a_backup() {
        use buwiz_protocol::broker_dir::{self, Move};
        let h = home("move");
        let old = grok_shell(&h);
        let chunks = old.join("chunks");
        let mut c = Config::parse(&args(&["--data", old.to_str().unwrap()]), None).unwrap();
        let moved = c.settle_owner_dir(None, chunks.to_str(), &h.0, &|| false);
        let new = broker_dir::new_dir(&h.0);
        let Some(Move::Moved { backup: Some(backup), .. }) = moved else { panic!("{moved:?}") };
        assert_eq!(c.data, Some(new.clone()));
        assert_eq!(c.data_dir(Some(old.to_str().unwrap())), new, "--data and the environment both follow");
        assert!(c.node_env.contains(&("NSH_CHUNK_DIR".to_owned(), new.join("chunks").to_string_lossy().into_owned())));
        assert!(old.symlink_metadata().unwrap().file_type().is_symlink(), "Grok Shell is left as a link");
        let copy = rusqlite::Connection::open(backup.join("app.db")).unwrap();
        assert_eq!(copy.query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0)).unwrap(), 42, "the backup holds the data");
        let live = rusqlite::Connection::open(new.join("generated/app.db")).unwrap();
        assert_eq!(live.query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0)).unwrap(), 42, "the data is unchanged");
        assert!(new.join(broker_dir::MOVED_MARKER).is_file());
        // the next start, given the new name, finds it there
        let mut c = Config::parse(&args(&[]), None).unwrap();
        assert_eq!(c.settle_owner_dir(new.to_str(), None, &h.0, &|| false), Some(Move::AlreadyThere));
        assert_eq!(c.data, Some(new));
    }

    /// Given the new name before anything moved, while a broker runs on
    /// the old folder: no move, and the old folder is used.
    #[test]
    fn a_running_broker_keeps_the_old_folder_in_use() {
        use buwiz_protocol::broker_dir::{self, Move};
        let h = home("busy");
        let old = grok_shell(&h);
        let new = broker_dir::new_dir(&h.0);
        let mut c = Config::parse(&args(&["--data", new.to_str().unwrap()]), None).unwrap();
        assert_eq!(c.settle_owner_dir(None, None, &h.0, &|| true), Some(Move::Busy));
        assert_eq!(c.data, Some(old.clone()));
        assert!(!new.exists() && old.symlink_metadata().unwrap().is_dir() && old.join("generated/app.db").is_file());
    }

    /// A test's or a bench's data dir is never the owner's: nothing is
    /// looked at, even with a Grok Shell beside it.
    #[test]
    fn another_data_dir_is_left_as_given() {
        let h = home("other");
        let old = grok_shell(&h);
        let bench = h.0.join("bench");
        let mut c = Config::parse(&args(&["--data", bench.to_str().unwrap()]), None).unwrap();
        assert_eq!(c.settle_owner_dir(None, None, &h.0, &|| false), None);
        let mut e = Config::parse(&args(&[]), None).unwrap();
        assert_eq!(e.settle_owner_dir(bench.to_str(), None, &h.0, &|| false), None);
        assert_eq!((c.data, e.data), (Some(bench), None));
        assert!(old.join("generated/app.db").is_file() && !buwiz_protocol::broker_dir::new_dir(&h.0).exists());
    }
}
