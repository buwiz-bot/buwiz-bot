//! The stdio MCP client the helper hosts plugins and servers with: the
//! `buwiz-mcp` crate (buwiz-gws in buwiz-app/google-workspace keeps a copy).

pub use buwiz_mcp::*;

#[cfg(test)]
mod tests {
    use super::{child_env_from, tool_path, Spec};

    fn spec(inherit_all: bool, env: &[(&str, &str)]) -> Spec {
        Spec { command: "x".into(), args: vec![], env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), inherit_all, client: "buwiz-broker" }
    }

    fn base() -> Vec<(String, String)> {
        vec![("HOME".into(), "/h".into()), ("PATH".into(), "/usr/bin:/bin".into()), ("USER".into(), "u".into())]
    }

    fn path(env: &[(String, String)]) -> Option<&str> {
        env.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v.as_str())
    }

    #[test]
    fn a_v1_plugin_gets_the_tool_folders_on_path_as_a_v0_server_does() {
        let env = child_env_from(&spec(false, &[("GWS_USER_EMAIL", "a@b")]), base());
        // what mcp_servers gives a v0 server, so a Dock-launched Rox's gws finds uvx
        assert_eq!(path(&env), Some(tool_path("/usr/bin:/bin", "/h").as_str()));
        assert_eq!(path(&env), Some("/usr/bin:/bin:/h/.local/bin:/h/.cargo/bin:/h/.bun/bin:/opt/homebrew/bin:/usr/local/bin"));
        assert_eq!(env.iter().filter(|(k, _)| k == "PATH").count(), 1);
        assert!(env.contains(&("GWS_USER_EMAIL".into(), "a@b".into())) && env.contains(&("USER".into(), "u".into())));
        // no PATH inherited: the tool folders alone
        assert_eq!(path(&child_env_from(&spec(false, &[]), vec![("HOME".into(), "/h".into())])), Some("/h/.local/bin:/h/.cargo/bin:/h/.bun/bin:/opt/homebrew/bin:/usr/local/bin"));
    }

    #[test]
    fn the_install_records_own_path_wins() {
        let env = child_env_from(&spec(false, &[("PATH", "/only")]), base());
        assert_eq!(path(&env), Some("/only"));
        assert_eq!(env.iter().filter(|(k, _)| k == "PATH").count(), 1);
    }

    #[test]
    fn a_v0_server_keeps_what_its_spec_says() {
        // v0 specs carry tool_path's PATH themselves (mcp_servers::connect)
        let env = child_env_from(&spec(true, &[]), base());
        assert_eq!(path(&env), Some("/usr/bin:/bin"));
    }
}
