//! A Bot's tool switches as the broker and the app both name them: the key of an MCP server or a
//! v1 plugin, and the owner of the plain chats' switches.

/// The owner id of the profile every plain chat shares (never a Bot's id: those are `b_…`).
pub const CHATS: &str = "@chats";

/// The switch key of MCP server `id`.
pub fn mcp_key(id: &str) -> String {
    format!("mcp:{id}")
}

/// The switch key of v1 plugin `id` (its full id).
pub fn v1_key(id: &str) -> String {
    format!("v1:{id}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn keys_are_prefixed_by_kind() {
        assert_eq!((super::mcp_key("apps"), super::v1_key("app.buwiz.books"), super::CHATS), ("mcp:apps".to_owned(), "v1:app.buwiz.books".to_owned(), "@chats"));
    }
}
