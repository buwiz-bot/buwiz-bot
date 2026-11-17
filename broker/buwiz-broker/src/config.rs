//! The command line and environment.
//!
//!   buwiz-broker [--port P] [--data DIR] [--assets DIR] [--no-node]
//!
//! The port comes from `--port`, else `PORT`, else 8787.
//! `--no-node` is accepted and does nothing: the app (and other repos' test
//! scripts) still pass the flag. It goes in a later release. `--node` and
//! `--node-script` are unknown arguments.
//! The same goes for the variables `BUWIZ_BROKER_HUB`, `BUWIZ_BROKER_V1_HOST`
//! and `BUWIZ_BROKER_MCP_HOST` (and their `ROX_` names): each is
//! accepted and ignored: the hub, the plugin host and the MCP host are always on. Other repos' test
//! scripts still set them.
//! `--assets` is the folder of the static files the broker serves and
//! reads (office pages and seeds, the plugin catalog and its plugins,
//! trust.json; `node_modules` beside it). Only the flag names it: without
//! it the broker has no plugin catalog, no office pages or seeds, and no
//! trust list unless `BUWIZ_TRUST_FILE` names one (a missing trust list
//! trusts nothing).
//! The data dir is `--data`, else `BUWIZ_BROKER_DATA_DIR`, else the working
//! directory.
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
    /// `--assets`: where the static files are; `None` has none.
    pub assets: Option<PathBuf>,
}

impl Config {
    pub fn parse(args: &[String], env_port: Option<&str>) -> Result<Self, String> {
        let mut port = env_port.map(str::to_owned);
        let mut data = None;
        let mut assets = None;
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let mut value = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
            match arg.as_str() {
                "--port" => port = Some(value("--port")?),
                "--data" => data = Some(PathBuf::from(value("--data")?)),
                "--assets" => assets = Some(PathBuf::from(value("--assets")?)),
                // accepted for one more release; it does nothing
                "--no-node" => {}
                other => return Err(format!("unknown argument {other}")),
            }
        }
        let port = match port.filter(|p| !p.is_empty()) {
            Some(p) => p.parse().map_err(|_| format!("the port {p} is not a number"))?,
            None => 8787,
        };
        Ok(Self { port, data, assets })
    }

    /// The folder of the static files (`--assets`), if one was named.
    pub fn assets_dir(&self) -> Option<&std::path::Path> {
        self.assets.as_deref()
    }

    /// The plugin trust store: `BUWIZ_TRUST_FILE`, else `trust.json` in the
    /// assets folder. `None` with neither: nothing is trusted. A named file
    /// that is missing trusts nothing either.
    pub fn trust_file(&self, env: Option<&str>) -> Option<PathBuf> {
        match env.filter(|e| !e.is_empty()) {
            Some(f) => Some(PathBuf::from(f)),
            None => self.assets_dir().map(|dir| dir.join("trust.json")),
        }
    }

    /// The data dir: `--data`, else `BUWIZ_BROKER_DATA_DIR`, else the working
    /// directory.
    pub fn data_dir(&self, env: Option<&str>) -> PathBuf {
        if let Some(data) = &self.data {
            return data.clone();
        }
        match env.filter(|e| !e.is_empty()) {
            Some(env) => PathBuf::from(env),
            None => PathBuf::from("."),
        }
    }

    /// `--assets` was given, no `BUWIZ_TRUST_FILE` is, and the assets folder
    /// has no `trust.json`: the broker starts and trusts nothing.
    pub fn assets_lack_a_trust_list(&self, trust_env: Option<&str>) -> bool {
        trust_env.is_none_or(str::is_empty) && self.assets_dir().is_some_and(|dir| !dir.join("trust.json").is_file())
    }

    /// What this start leaves off for want of `--assets`: the plugin catalog
    /// and its bundled plugins, the office pages and seeds, and (unless
    /// `BUWIZ_TRUST_FILE` names one) the trust list. Empty when it was given.
    pub fn off_without_assets(&self, trust_env: Option<&str>) -> Vec<&'static str> {
        if self.assets.is_some() {
            return Vec::new();
        }
        let mut off = vec!["the plugin catalog", "the office pages and seeds"];
        if self.trust_file(trust_env).is_none() {
            off.push("the plugin trust list");
        }
        off
    }
}

impl Config {
    /// When the data dir (`--data`, else `env_data`) is the owner's folder
    /// under `home`, by either name: moves `Grok Shell` to `Buwiz Broker`
    /// once (`busy`: a broker answers on the owner's port) and points `data` at the folder holding the
    /// data. `None` for any other data dir: nothing is looked at.
    pub fn settle_owner_dir(
        &mut self,
        env_data: Option<&str>,
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
    fn the_data_dir_is_the_flag_then_the_environment_then_the_working_directory() {
        let c = Config::parse(&args(&[]), None).unwrap();
        assert_eq!(c.data_dir(None), PathBuf::from("."));
        assert_eq!(c.data_dir(Some("")), PathBuf::from("."));
        assert_eq!(c.data_dir(Some("/env")), PathBuf::from("/env"));
        let c = Config::parse(&args(&["--data", "/d"]), None).unwrap();
        assert_eq!(c.data_dir(Some("/env")), PathBuf::from("/d"));
    }

    #[test]
    fn the_node_flags_are_gone_and_no_node_is_accepted_and_does_nothing() {
        for flag in [&["--node", "node"][..], &["--node-script", "app-server.mjs"][..]] {
            let err = Config::parse(&args(flag), None).unwrap_err();
            assert_eq!(err, format!("unknown argument {}", flag[0]));
        }
        assert_eq!(Config::parse(&args(&["--no-node"]), None).unwrap(), Config::parse(&args(&[]), None).unwrap());
        let c = Config::parse(&args(&["--data", "/d", "--no-node", "--assets", "/a"]), None).unwrap();
        assert_eq!((c.data, c.assets), (Some(PathBuf::from("/d")), Some(PathBuf::from("/a"))));
    }

    #[test]
    fn the_static_files_come_from_the_assets_flag_and_only_from_there() {
        let d = Config::parse(&args(&[]), None).unwrap();
        assert_eq!(d.assets_dir(), None);
        assert_eq!(d.trust_file(None), None);
        assert_eq!(d.trust_file(Some("")), None);
        assert_eq!(d.trust_file(Some("/t/trust.json")), Some(PathBuf::from("/t/trust.json")));
        let c = Config::parse(&args(&["--assets", "/b/broker/server"]), None).unwrap();
        assert_eq!(c.assets_dir(), Some(std::path::Path::new("/b/broker/server")));
        // the trust store is the assets' own, unless the environment names one
        assert_eq!(c.trust_file(None), Some(PathBuf::from("/b/broker/server/trust.json")));
        assert_eq!(c.trust_file(Some("/t/trust.json")), Some(PathBuf::from("/t/trust.json")));
        assert!(Config::parse(&args(&["--assets"]), None).is_err());
    }

    #[test]
    fn without_assets_the_start_lists_what_is_off() {
        let d = Config::parse(&args(&[]), None).unwrap();
        assert_eq!(d.off_without_assets(None), ["the plugin catalog", "the office pages and seeds", "the plugin trust list"]);
        assert_eq!(d.off_without_assets(Some("/t/trust.json")), ["the plugin catalog", "the office pages and seeds"]);
        let c = Config::parse(&args(&["--assets", "/a"]), None).unwrap();
        assert!(c.off_without_assets(None).is_empty());
    }

    #[test]
    fn assets_with_no_trust_list_are_noticed() {
        let dir = std::env::temp_dir().join(format!("buwiz-broker-config-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let c = Config::parse(&args(&["--assets", dir.to_str().unwrap()]), None).unwrap();
        assert!(c.assets_lack_a_trust_list(None));
        assert!(c.assets_lack_a_trust_list(Some("")));
        assert!(!c.assets_lack_a_trust_list(Some("/t/trust.json")), "a named file is the environment's to answer for");
        std::fs::write(dir.join("trust.json"), "{}").unwrap();
        assert!(!c.assets_lack_a_trust_list(None));
        // no --assets is the other case, listed by off_without_assets
        assert!(!Config::parse(&args(&[]), None).unwrap().assets_lack_a_trust_list(None));
        let _ = std::fs::remove_dir_all(&dir);
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
    /// moves it and runs on the new one.
    #[test]
    fn the_owners_folder_moves_once_with_a_backup() {
        use buwiz_protocol::broker_dir::{self, Move};
        let h = home("move");
        let old = grok_shell(&h);
        let mut c = Config::parse(&args(&["--data", old.to_str().unwrap()]), None).unwrap();
        let moved = c.settle_owner_dir(None, &h.0, &|| false);
        let new = broker_dir::new_dir(&h.0);
        let Some(Move::Moved { backup: Some(backup), .. }) = moved else { panic!("{moved:?}") };
        assert_eq!(c.data, Some(new.clone()));
        assert_eq!(c.data_dir(Some(old.to_str().unwrap())), new, "--data and the environment both follow");
        assert!(old.symlink_metadata().unwrap().file_type().is_symlink(), "Grok Shell is left as a link");
        let copy = rusqlite::Connection::open(backup.join("app.db")).unwrap();
        assert_eq!(copy.query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0)).unwrap(), 42, "the backup holds the data");
        let live = rusqlite::Connection::open(new.join("generated/app.db")).unwrap();
        assert_eq!(live.query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0)).unwrap(), 42, "the data is unchanged");
        assert!(new.join(broker_dir::MOVED_MARKER).is_file());
        // the next start, given the new name, finds it there
        let mut c = Config::parse(&args(&[]), None).unwrap();
        assert_eq!(c.settle_owner_dir(new.to_str(), &h.0, &|| false), Some(Move::AlreadyThere));
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
        assert_eq!(c.settle_owner_dir(None, &h.0, &|| true), Some(Move::Busy));
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
        assert_eq!(c.settle_owner_dir(None, &h.0, &|| false), None);
        let mut e = Config::parse(&args(&[]), None).unwrap();
        assert_eq!(e.settle_owner_dir(bench.to_str(), &h.0, &|| false), None);
        assert_eq!((c.data, e.data), (Some(bench), None));
        assert!(old.join("generated/app.db").is_file() && !buwiz_protocol::broker_dir::new_dir(&h.0).exists());
    }
}
