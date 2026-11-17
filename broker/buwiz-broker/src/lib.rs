//! buwiz-broker: the broker's front door.
//!
//! It owns the broker's port and answers `/health`, `/meta` and every route
//! itself: `leaf`, the line and the MCP routes, and with `BUWIZ_BROKER_HUB=1`
//! the brain's (`hub`). It forwards nothing.

pub mod config;
pub mod front_door;
pub mod guard;
pub mod helper;
pub mod hub;
pub mod js;
pub mod leaf;
pub mod office;
pub mod v1;

#[cfg(test)]
#[path = "../../test-support/pins.rs"]
pub(crate) mod pins;

/// A request path as the log shows it: no query, and the secret of a
/// webhook (`/v1/hooks/<id>/<secret>`) replaced.
pub fn redact(path_and_query: &str) -> String {
    let path = path_and_query.split('?').next().unwrap_or("");
    match path.strip_prefix("/v1/hooks/") {
        Some(rest) => match rest.split_once('/') {
            Some((id, _)) => format!("/v1/hooks/{id}/***"),
            None => path.to_owned(),
        },
        None => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn the_log_keeps_no_query_and_no_webhook_secret() {
        assert_eq!(redact("/threads"), "/threads");
        assert_eq!(redact("/turns?thread=t5"), "/turns");
        assert_eq!(redact("/v1/hooks/ah_abc/s3cr3t"), "/v1/hooks/ah_abc/***");
        assert_eq!(redact("/v1/hooks/ri_x/s3cr3t?x=1"), "/v1/hooks/ri_x/***");
        assert_eq!(redact("/v1/hooks/"), "/v1/hooks/");
        assert_eq!(redact("/mcp/open/pass?token=abc"), "/mcp/open/pass");
    }
}
