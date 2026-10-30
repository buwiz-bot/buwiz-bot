//! An app's `permissions` (spec 5.6) compiled into what its sandbox has
//! (6.4): the bridge methods it gets, and an nsh policy that grants the
//! `host` capability for exactly those names. Nothing else is granted:
//! no network, web views, clipboard, picker, files, processes, window or
//! home directory. A permission not granted has no handler, no policy
//! name and no shim method: absent, not failing.
//!
//! Parsing is strict (spec 2.2): an unknown key inside `permissions` is a
//! refusal, since it would be an access grant this host does not know.

use serde_json::Value;

use super::RoxError;

/// The spec's limits on permission values (Appendix A).
const MAX_TOOLS: usize = 64;
const MAX_OPEN_APPS: usize = 16;
const MAX_EXTERNAL: usize = 8;
const MAX_ATTACH: usize = 16;
const MAX_EXTENSIONS: usize = 16;
/// 50 MiB.
pub const MAX_PICK_BYTES: u64 = 52_428_800;
/// 10 MiB.
pub const MAX_QUOTA: u64 = 10_485_760;
/// 1 MiB.
pub const DEFAULT_QUOTA: u64 = 1_048_576;
/// 8 MiB: the largest `rox.preview` document, and its default.
pub const MAX_PREVIEW: u64 = 8_388_608;

/// What one app may reach.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Permissions {
    pub tools: Vec<String>,
    pub open: Option<OpenGrant>,
    pub attach: Vec<String>,
    pub pick_file: Option<PickGrant>,
    /// `quota_bytes`, when storage is granted.
    pub storage: Option<u64>,
    pub copy: bool,
    /// `max_bytes` of a `rox.preview` document, when previews are granted.
    pub preview: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpenGrant {
    pub apps: Vec<String>,
    /// Lower-case `https://host[:port]` origins.
    pub external: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PickGrant {
    /// Lower-case, with the leading dot.
    pub extensions: Vec<String>,
    pub max_bytes: u64,
    pub multiple: bool,
}

/// The bridge's method names, as the shim and the policy know them.
pub const CALL: &str = "rox.call";
pub const OPEN: &str = "rox.open";
pub const ATTACH: &str = "rox.attach";
pub const PICK: &str = "rox.pickFile";
pub const COPY: &str = "rox.copy";
pub const PREVIEW: &str = "rox.preview";
pub const TICKET_TITLE: &str = "rox.ticket.title";
/// An inline app's content height (a post; 6.5, 7.11).
pub const FIT: &str = "rox.fit";
/// An inline app's data is in: its card may show (a post; an optional
/// addition under 1.0 that apps feature-detect, `rox.ready?.()`).
pub const READY: &str = "rox.ready";
/// An inline card's chrome, drawn by Rox (7.12 to 7.14, protocol 1.1):
/// its footer buttons, the outcome it settles to, its header status.
pub const ACTIONS: &str = "rox.actions";
pub const SETTLE: &str = "rox.settle";
pub const META: &str = "rox.meta";
/// The methods every app has, whatever it is granted.
pub const ALWAYS: [&str; 6] = [TICKET_TITLE, FIT, READY, ACTIONS, SETTLE, META];
pub const STORAGE: [&str; 4] = ["rox.storage.get", "rox.storage.set", "rox.storage.remove", "rox.storage.keys"];

impl Permissions {
    /// Parses `apps[].permissions` (absent: nothing).
    pub fn parse(v: Option<&Value>) -> Result<Permissions, RoxError> {
        let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(Permissions::default()) };
        let o = object(v, "permissions")?;
        let mut p = Permissions::default();
        for (k, v) in o {
            match k.as_str() {
                "tools" => p.tools = strings(v, "tools", MAX_TOOLS, tool_ok)?,
                "open" => p.open = Some(open(v)?),
                "attach" => p.attach = strings(v, "attach", MAX_ATTACH, scope_ok)?,
                "pick_file" => p.pick_file = Some(pick(v)?),
                "storage" => p.storage = Some(storage(v)?),
                "copy" => p.copy = v.as_bool().ok_or_else(|| invalid("copy is a boolean"))?,
                "preview" => p.preview = Some(preview(v)?),
                _ => return Err(invalid(&format!("unknown permission {k:?}"))),
            }
        }
        Ok(p)
    }

    /// The bridge methods granted (6.4), [`ALWAYS`]'s always.
    pub fn methods(&self) -> Vec<&'static str> {
        let mut m = ALWAYS.to_vec();
        if !self.tools.is_empty() {
            m.push(CALL);
        }
        if self.open.is_some() {
            m.push(OPEN);
        }
        if !self.attach.is_empty() {
            m.push(ATTACH);
        }
        if self.pick_file.is_some() {
            m.push(PICK);
        }
        if self.storage.is_some() {
            m.extend(STORAGE);
        }
        if self.copy {
            m.push(COPY);
        }
        if self.preview.is_some() {
            m.push(PREVIEW);
        }
        m
    }

    /// The app's own policy: deny-all, hardened, plus `host` for exactly
    /// [`Permissions::methods`]. Made per open, never shared (6.4).
    pub fn policy(&self) -> nsh_engine::Policy {
        let host: Vec<Value> = self.methods().into_iter().map(Value::from).collect();
        let declared = nsh_core::access::Capabilities::from_json(&serde_json::json!({ "host": host }))
            .expect("bridge method names are valid host names");
        nsh_engine::Policy { harden: true, declared: Some(declared), granted: None }
    }
}

fn invalid(m: &str) -> RoxError {
    RoxError::new("invalid_args", m)
}

fn object<'a>(v: &'a Value, what: &str) -> Result<&'a serde_json::Map<String, Value>, RoxError> {
    v.as_object().ok_or_else(|| invalid(&format!("{what} is an object")))
}

fn strings(v: &Value, what: &str, max: usize, ok: fn(&str) -> bool) -> Result<Vec<String>, RoxError> {
    let items = v.as_array().ok_or_else(|| invalid(&format!("{what} is an array")))?;
    if items.len() > max {
        return Err(invalid(&format!("{what} has more than {max} entries")));
    }
    items
        .iter()
        .map(|x| match x.as_str() {
            Some(s) if ok(s) => Ok(s.to_owned()),
            _ => Err(invalid(&format!("bad entry in {what}: {x}"))),
        })
        .collect()
}

fn open(v: &Value) -> Result<OpenGrant, RoxError> {
    let mut g = OpenGrant::default();
    for (k, v) in object(v, "open")? {
        match k.as_str() {
            "apps" => g.apps = strings(v, "open.apps", MAX_OPEN_APPS, app_id_ok)?,
            "external" => {
                g.external = strings(v, "open.external", MAX_EXTERNAL, |s| https_origin(s).as_deref() == Some(s))?
            }
            _ => return Err(invalid(&format!("unknown key open.{k}"))),
        }
    }
    Ok(g)
}

fn pick(v: &Value) -> Result<PickGrant, RoxError> {
    let mut g = PickGrant { max_bytes: MAX_PICK_BYTES, ..Default::default() };
    for (k, v) in object(v, "pick_file")? {
        match k.as_str() {
            "extensions" => {
                g.extensions = strings(v, "pick_file.extensions", MAX_EXTENSIONS, extension_ok)?
                    .into_iter()
                    .map(|e| e.to_ascii_lowercase())
                    .collect()
            }
            "max_bytes" => match v.as_u64() {
                Some(n) if n <= MAX_PICK_BYTES => g.max_bytes = n,
                _ => return Err(invalid("pick_file.max_bytes is at most 50 MiB")),
            },
            "multiple" => g.multiple = v.as_bool().ok_or_else(|| invalid("pick_file.multiple is a boolean"))?,
            _ => return Err(invalid(&format!("unknown key pick_file.{k}"))),
        }
    }
    Ok(g)
}

fn storage(v: &Value) -> Result<u64, RoxError> {
    let mut quota = DEFAULT_QUOTA;
    for (k, v) in object(v, "storage")? {
        match (k.as_str(), v.as_u64()) {
            ("quota_bytes", Some(n)) if n <= MAX_QUOTA => quota = n,
            ("quota_bytes", _) => return Err(invalid("storage.quota_bytes is at most 10 MiB")),
            _ => return Err(invalid(&format!("unknown key storage.{k}"))),
        }
    }
    Ok(quota)
}

fn preview(v: &Value) -> Result<u64, RoxError> {
    let mut max = MAX_PREVIEW;
    for (k, v) in object(v, "preview")? {
        match (k.as_str(), v.as_u64()) {
            ("max_bytes", Some(n)) if (1..=MAX_PREVIEW).contains(&n) => max = n,
            ("max_bytes", _) => return Err(invalid("preview.max_bytes is at most 8 MiB")),
            _ => return Err(invalid(&format!("unknown key preview.{k}"))),
        }
    }
    Ok(max)
}

/// `https://host[:port]` of an https URL, lower-cased; `None` for any other
/// scheme or a URL with credentials.
pub fn https_origin(url: &str) -> Option<String> {
    let rest = url.get(..8).filter(|s| s.eq_ignore_ascii_case("https://")).map(|_| &url[8..])?;
    let auth = rest.split(['/', '?', '#']).next()?;
    let ok = !auth.is_empty()
        && !auth.contains('@')
        && auth.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'));
    ok.then(|| format!("https://{}", auth.to_ascii_lowercase()))
}

// The spec's grammars (section 3), shared with the broker (rox-v1).
pub use rox_v1::ids::{app_id_ok, plugin_id_ok, prop_key_ok, scope_ok, short_ok, ticket_ok, tool_ok};

/// A file extension: `.` and 1 to 15 letters or digits.
fn extension_ok(s: &str) -> bool {
    s.strip_prefix('.').is_some_and(|e| (1..=15).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn fixture_board() -> Value {
        json!({
            "tools": ["fx_echo", "fx_slow"],
            "open": { "apps": ["card"], "external": ["https://example.com"] },
            "attach": ["item"],
            "pick_file": { "extensions": [".XLSX", ".csv"], "max_bytes": 26214400, "multiple": false },
            "storage": { "quota_bytes": 1048576 },
            "copy": true,
            "preview": {}
        })
    }

    #[test]
    fn permissions_parse_strictly() {
        let p = Permissions::parse(Some(&fixture_board())).unwrap();
        assert_eq!(p.pick_file.as_ref().unwrap().extensions, [".xlsx", ".csv"]);
        assert_eq!(p.storage, Some(1048576));
        assert_eq!(p.preview, Some(MAX_PREVIEW));
        let capped = Permissions::parse(Some(&json!({"preview": {"max_bytes": 2048}}))).unwrap();
        assert_eq!(capped.preview, Some(2048));
        assert_eq!(Permissions::parse(None).unwrap(), Permissions::default());
        // MAN-06: an unknown key in permissions, or in a grant inside it, is refused
        for bad in [
            json!({"network": true}),
            json!({"open": {"apps": [], "urls": []}}),
            json!({"storage": {"quota_bytes": 10485761}}),
            json!({"pick_file": {"max_bytes": 52428801}}),
            json!({"open": {"external": ["http://example.com"]}}),
            json!({"open": {"external": ["https://example.com/path"]}}),
            json!({"tools": ["rox.provision"]}),
            json!({"copy": "yes"}),
            json!({"preview": true}),
            json!({"preview": {"scripts": true}}),
            json!({"preview": {"max_bytes": 8388609}}),
            json!({"preview": {"max_bytes": 0}}),
        ] {
            assert!(Permissions::parse(Some(&bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn app_07_policy_grants_host_for_granted_methods_only() {
        let p = Permissions::parse(Some(&json!({"tools": ["fx_echo"]}))).unwrap();
        assert_eq!(p.methods(), ["rox.ticket.title", "rox.fit", "rox.ready", "rox.actions", "rox.settle", "rox.meta", "rox.call"]);
        let access = p.policy().access();
        for always in ["rox.call", "rox.ticket.title", "rox.fit", "rox.ready", "rox.actions", "rox.settle", "rox.meta"] {
            assert!(access.allows_host(always), "{always}");
        }
        for absent in ["rox.open", "rox.attach", "rox.pickFile", "rox.copy", "rox.preview", "rox.storage.get", "anything"] {
            assert!(!access.allows_host(absent), "{absent}");
        }
        assert!(!access.allows_http("GET", "http://127.0.0.1:8787/v1/host/call"));
        assert!(!access.allows_http("POST", "https://example.com/"));
        assert!(!access.allows_websocket("ws://127.0.0.1:8787/"));
        assert!(!access.allows_socket("127.0.0.1", 8787));
        assert!(!access.allows_media("https://example.com/x.png"));
        assert!(!access.allows_webviews());
        assert!(!access.allows_clipboard());
        assert!(!access.allows_picker());
        assert!(!access.allows_status_bar());
        assert!(!access.allows_open_url("https://example.com/"));
        assert!(!access.allows_process("sh"));
        assert!(p.policy().harden);
        let all = Permissions::parse(Some(&fixture_board())).unwrap();
        assert_eq!(all.methods().len(), 16);
        assert!(all.policy().access().allows_host("rox.preview"));
        // a preview is the host's own web view, not one the app gets
        assert!(!all.policy().access().allows_webviews());
        // copy granted is the bridge's pasteboard write, not nsh's clipboard
        assert!(!all.policy().access().allows_clipboard());
    }

    #[test]
    fn grammars_take_the_specs_examples() {
        assert!(plugin_id_ok("dev.goldcoders.bir"));
        assert!(!plugin_id_ok("bir") && !plugin_id_ok("dev.Goldcoders.bir") && !plugin_id_ok("dev..bir"));
        assert!(app_id_ok("tax-profile") && !app_id_ok("Tax") && !app_id_ok(&"a".repeat(49)));
        assert!(tool_ok("bir_form_open") && !tool_ok("rox.provision"));
        assert!(short_ok("bir") && !short_ok("b") && !short_ok("bir-x"));
        assert!(ticket_ok("tkt_0123456789ABCDEFGHJKMNPQRS"));
        assert!(!ticket_ok("tkt_0123456789ABCDEFGHIJKLMNOP"), "I, L, O and U are not Crockford");
        assert!(prop_key_ok("ticket") && !prop_key_ok("_x") && !prop_key_ok(&"a".repeat(33)));
        assert_eq!(https_origin("https://Example.com:8443/a?b").as_deref(), Some("https://example.com:8443"));
        assert_eq!(https_origin("http://example.com"), None);
        assert_eq!(https_origin("https://u@example.com/"), None);
    }
}
