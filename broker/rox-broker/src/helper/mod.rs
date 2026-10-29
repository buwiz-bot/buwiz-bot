//! The Mac helper (plan phases 3 and 4, rule R7): what has to run on this
//! Mac (plugin keys, plugin processes, MCP children, local tools), behind
//! an in-process API shaped like the line, so a remote Hub needs no rework.

pub mod keystore;
pub mod line;
pub mod mcp;
pub mod mcp_servers;
pub mod plugins;
pub mod sessions;
