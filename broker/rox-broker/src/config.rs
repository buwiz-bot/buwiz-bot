//! The command line and environment.
//!
//!   rox-broker [--port P] [--data DIR] [--node PATH] [--node-script PATH]
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
//! `--data` sets `GROK_DATA_DIR` for Node; without it Node keeps the
//! environment's. The routes answered in Rust use the same data dir as
//! Node (`data_dir`).

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

    /// The trust store Node reads (v1/trust.mjs): `ROX_TRUST_FILE`, else
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

    /// Node's data dir: `--data`, else `GROK_DATA_DIR`, else the folder
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
}
