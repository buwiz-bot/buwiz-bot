//! `rox://app/` links of v1 plugins (spec 9.4): which links are v1's, the
//! limits (INV-07), the `rox://app/bir/...` alias for BIR's v1 plugin
//! (16 item 1), a plugin that is not installed (13.6), and the notice an
//! open shows when its ticket no longer resolves (INV-06).
//!
//! A link names the plugin by its full id. Until a plugin moves to v1 its
//! v0 links (`rox://app/<server>/<app>`, a server's local name) go the v0
//! way, unchanged: only a link naming a reverse-DNS plugin id, or `bir`
//! once `dev.goldcoders.bir` is installed as a v1 plugin, is v1's.

use super::{RoxError, V1Key, host_api, load::PluginEntry, perms};

/// The longest link (9.4, Appendix A).
pub const MAX_LINK: usize = 2048;
/// The most `props` a link carries, encoded (9.4).
pub const MAX_LINK_PROPS: usize = 1024;
/// BIR's v1 plugin id, which `rox://app/bir/...` names for one minor
/// release (16 item 1).
pub const BIR: &str = "dev.goldcoders.bir";
/// What an open whose ticket does not resolve shows (9.4).
pub const NO_LONGER_AVAILABLE: &str = "This item is no longer available";

/// Where a `rox://app/` link goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// Not a v1 plugin's link: the v0 way, unchanged.
    NotV1,
    /// A v1 link that breaks a rule: never opened.
    Refused(String),
    /// Its plugin is not installed: a "not installed" note and nothing else.
    NotInstalled(String),
    /// Its plugin is disabled (13.4): a note and nothing else.
    Disabled(String),
    /// The app to open (after the consent its opener needs, 9.7).
    Open(V1Key),
}

/// The plugin id a link's first segment names among `installed`: a
/// plugin id as it is, `bir` when BIR's v1 plugin is installed.
fn plugin_of(segment: &str, installed: &[PluginEntry]) -> Option<String> {
    if perms::plugin_id_ok(segment) {
        return Some(segment.to_owned());
    }
    (segment == "bir" && installed.iter().any(|p| p.id == BIR)).then(|| BIR.to_owned())
}

/// Routes `url` among the v1 plugins `installed`.
pub fn route(url: &str, installed: &[PluginEntry]) -> Route {
    let Some(rest) = url.strip_prefix("rox://app/") else { return Route::NotV1 };
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut parts = path.trim_end_matches('/').splitn(2, '/');
    let (Some(first), Some(app)) = (parts.next(), parts.next()) else { return Route::NotV1 };
    let Some(plugin) = plugin_of(first, installed) else { return Route::NotV1 };
    if url.len() > MAX_LINK {
        return Route::Refused(format!("a link is at most {MAX_LINK} characters"));
    }
    let props = query.split('&').find_map(|kv| kv.strip_prefix("props="));
    if props.is_some_and(|p| p.len() > MAX_LINK_PROPS) {
        return Route::Refused(format!("a link's props are at most {MAX_LINK_PROPS} bytes"));
    }
    // only the ticket and props cross: v0's `token=` (a TIN) is dropped
    let query: Vec<&str> = query.split('&').filter(|kv| kv.starts_with("ticket=") || kv.starts_with("props=")).collect();
    let text = if query.is_empty() { format!("v1:{plugin}/{app}") } else { format!("v1:{plugin}/{app}?{}", query.join("&")) };
    let key = match V1Key::parse(&text) {
        Ok(k) => k,
        Err(e) => return Route::Refused(e.message),
    };
    match installed.iter().find(|p| p.id == key.plugin) {
        None => Route::NotInstalled(key.plugin),
        Some(p) if !p.enabled => Route::Disabled(p.title.clone()),
        Some(_) => Route::Open(key),
    }
}

/// `key`'s link: `rox://app/<plugin id>/<app id>[?ticket=][&props=]`.
pub fn link_of(key: &V1Key) -> String {
    let text = key.to_key();
    format!("rox://app/{}", text.strip_prefix("v1:").unwrap_or(&text))
}

/// "<plugin> is not installed" (the 13.6 card).
pub fn not_installed(plugin: &str) -> String {
    format!("{plugin} is not installed")
}

/// The notice an open of `key` shows: [`NO_LONGER_AVAILABLE`] when its
/// ticket does not resolve (the plugin says `valid: false`), none when it
/// has no ticket or the plugin could not be asked (the app says so then).
pub fn ticket_notice(broker: &dyn host_api::Broker, key: &V1Key) -> Option<&'static str> {
    let ticket = key.ticket.as_deref()?;
    match host_api::ticket_title(broker, &key.plugin, ticket) {
        Ok(None) => Some(NO_LONGER_AVAILABLE),
        Ok(Some(_)) => None,
        Err(RoxError { code, .. }) if code == "not_supported" => None,
        Err(e) => {
            log::info!("mini v1: ticket of {}: {e}", key.plugin);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::*;

    const T: &str = "tkt_0123456789ABCDEFGHJKMNPQRS";

    fn entry(id: &str, enabled: bool) -> PluginEntry {
        PluginEntry::from_json(&json!({"id": id, "short": "bir", "title": "BIR", "enabled": enabled, "apps": [{"id": "forms", "title": "BIR Forms"}]})).unwrap()
    }

    #[test]
    fn a_v1_link_names_its_plugin_app_ticket_and_props() {
        let installed = [entry(BIR, true)];
        let url = format!("rox://app/dev.goldcoders.bir/forms?ticket={T}&props=%7B%22form%22%3A%221601C%22%7D");
        let Route::Open(k) = route(&url, &installed) else { panic!("{:?}", route(&url, &installed)) };
        assert_eq!((k.plugin.as_str(), k.app.as_str(), k.ticket.as_deref()), (BIR, "forms", Some(T)));
        assert_eq!(k.props["form"], "1601C");
        assert_eq!(route(&link_of(&k), &installed), Route::Open(k));
    }

    #[test]
    fn v0_links_stay_v0_for_v0_plugins() {
        // no v1 BIR installed: `rox://app/bir/...` is v0's
        assert_eq!(route("rox://app/bir/forms?token=%40bir%3Ataxprofile%3A123", &[]), Route::NotV1);
        assert_eq!(route("rox://app/bir/forms", &[entry("dev.goldcoders.other", true)]), Route::NotV1);
        assert_eq!(route("rox://chat/new", &[]), Route::NotV1);
        assert_eq!(route("rox://app/bir", &[]), Route::NotV1);
    }

    #[test]
    fn the_bir_alias_names_birs_v1_plugin_and_drops_v0_tokens() {
        let installed = [entry(BIR, true)];
        let Route::Open(k) = route(&format!("rox://app/bir/forms?ticket={T}&token=%40bir%3Ataxprofile%3A123"), &installed) else {
            panic!()
        };
        assert_eq!((k.plugin.as_str(), k.ticket.as_deref()), (BIR, Some(T)));
        assert!(!k.to_key().contains("token") && k.props.is_empty(), "a TIN never reaches the key");
    }

    #[test]
    fn inv_07_a_link_over_2048_characters_or_with_props_over_1024_bytes_is_refused() {
        let installed = [entry(BIR, true)];
        let base = "rox://app/dev.goldcoders.bir/forms?props=";
        // props that fit, link exactly at the limit: opens (the props are
        // checked against the app's schema when it is prepared)
        let props = format!("%7B%22a%22%3A%22{}%22%7D", "x".repeat(1024 - 22));
        assert_eq!(props.len(), 1024);
        assert!(matches!(route(&format!("{base}{props}"), &installed), Route::Open(_)));
        let over = format!("%7B%22a%22%3A%22{}%22%7D", "x".repeat(1024 - 21));
        assert!(matches!(route(&format!("{base}{over}"), &installed), Route::Refused(m) if m.contains("1024")));
        let head = format!("rox://app/dev.goldcoders.bir/forms?ticket={T}&x=");
        let at_limit = format!("{head}{}", "y".repeat(2048 - head.len()));
        assert_eq!(at_limit.len(), 2048);
        assert!(matches!(route(&at_limit, &installed), Route::Open(_)));
        let long = format!("{at_limit}y");
        assert_eq!(long.len(), 2049);
        assert!(matches!(route(&long, &installed), Route::Refused(m) if m.contains("2048")));
        // a ticket outside the grammar or props that are no object
        assert!(matches!(route("rox://app/dev.goldcoders.bir/forms?ticket=123456789", &installed), Route::Refused(_)));
        assert!(matches!(route("rox://app/dev.goldcoders.bir/forms?props=%5B%5D", &installed), Route::Refused(_)));
    }

    #[test]
    fn lcy_06_a_link_to_an_uninstalled_plugin_is_not_installed_and_nothing_else() {
        let r = route("rox://app/dev.goldcoders.gone/forms", &[entry(BIR, true)]);
        assert_eq!(r, Route::NotInstalled("dev.goldcoders.gone".to_owned()));
        assert_eq!(not_installed("dev.goldcoders.gone"), "dev.goldcoders.gone is not installed");
        assert_eq!(route("rox://app/dev.goldcoders.bir/forms", &[entry(BIR, false)]), Route::Disabled("BIR".to_owned()));
    }

    struct Titles;

    impl host_api::Broker for Titles {
        fn request(&self, _: &str, route: &str, body: Option<Value>, _: Duration) -> Result<Value, RoxError> {
            assert_eq!(route, "/v1/host/tickets/title");
            let b = body.unwrap();
            if b["ticket"] == T {
                Ok(json!({"ok": true, "valid": true, "title": "Dela Cruz Jane"}))
            } else {
                Ok(json!({"ok": true, "valid": false}))
            }
        }
    }

    #[test]
    fn inv_06_a_link_whose_ticket_does_not_resolve_opens_with_a_notice() {
        let mut k = V1Key::parse(&format!("v1:{BIR}/forms?ticket={T}")).unwrap();
        assert_eq!(ticket_notice(&Titles, &k), None);
        k.ticket = Some("tkt_ZZZZZZZZZZZZZZZZZZZZZZZZZZ".to_owned());
        assert_eq!(ticket_notice(&Titles, &k), Some("This item is no longer available"));
        k.ticket = None;
        assert_eq!(ticket_notice(&Titles, &k), None);
    }
}
