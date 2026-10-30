//! The Mac helper (plan phases 3 and 4, rule R7): what has to run on this
//! Mac (plugin keys, plugin processes, MCP children, local tools), behind
//! an in-process API shaped like the line, so a remote Hub needs no rework.

pub mod http;
pub mod keystore;
pub mod line;
pub mod local;
pub mod mcp;
pub mod mcp_servers;
pub mod oauth;
pub mod office;
pub mod plugins;
pub mod remote;
pub mod sessions;
