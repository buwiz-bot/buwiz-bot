//! What Rox keeps for v1 plugins in its own data dir
//! (`<data dir>/mini-v1.json`): the chats' attachments (10.3), the pins
//! (12.3), the fingerprint each app was last opened with (12.2) and
//! whether the one-time v0 migration ran (Q11).
//!
//! - An attachment is a canonical token plus its plugin id, the plugin's
//!   major version when it was attached, the scope, the title then and
//!   the chat. A plugin whose major changed since (or a migration that
//!   could not issue a ticket) makes it "needs update": it stays, and is
//!   not sent to the model (AGT-06).
//! - A pin is `{plugin id, app id, ticket?, placement}` and a title; no
//!   fingerprint (opening a pin opens the app's current version).
//! - Uninstalling a plugin removes everything here that names it (13.6).
//!
//! Nothing here holds a typed identifier: tokens are canonical (tickets).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{load::PluginEntry, tokens};

/// One chat's attachment (10.3).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Attachment {
    /// The canonical token; empty for a migrated item that got no ticket.
    pub token: String,
    pub plugin: String,
    pub major: u64,
    pub scope: String,
    pub title: String,
    pub thread: String,
    /// Set by a migration that could not issue a ticket (Q11).
    pub needs_update: bool,
}

/// A pin (12.3).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Pin {
    pub plugin: String,
    pub app: String,
    pub ticket: Option<String>,
    pub placement: String,
    pub title: String,
    /// Set by a migration that could not issue its ticket (Q11).
    pub needs_update: bool,
}

/// How an attachment or pin stands against the plugins installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Ok,
    /// "needs update": shown so, not sent to the model.
    NeedsUpdate,
    /// "<plugin title> is disabled" (13.4).
    Disabled(String),
    NotInstalled,
}

impl Status {
    /// What the chip says next to its title, "" when it is fine.
    pub fn label(&self) -> String {
        match self {
            Status::Ok => String::new(),
            Status::NeedsUpdate => "needs update".to_owned(),
            Status::Disabled(t) => format!("{t} is disabled"),
            Status::NotInstalled => "not installed".to_owned(),
        }
    }
}

/// The major of a semver `version` ("1.4.2" → 1).
pub fn major(version: &str) -> u64 {
    version.split(['.', '-', '+']).next().and_then(|m| m.parse().ok()).unwrap_or(0)
}

/// Everything in `mini-v1.json`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Records {
    pub attachments: Vec<Attachment>,
    pub pins: Vec<Pin>,
    /// `"<plugin>/<app>"` → the fingerprint last opened.
    pub opened: Vec<(String, String)>,
    /// When the v0 migration ran (ms), 0 if it has not.
    pub migrated_at: i64,
}

/// The file in `dir` (Rox's data dir).
pub fn file(dir: &Path) -> PathBuf {
    dir.join("mini-v1.json")
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned()
}

impl Records {
    /// The records in `dir`; none yet (or unreadable) is empty.
    pub fn load(dir: &Path) -> Records {
        let Ok(text) = std::fs::read_to_string(file(dir)) else { return Records::default() };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            log::warn!("mini v1: {} is not JSON; starting empty", file(dir).display());
            return Records::default();
        };
        let list = |k: &str| v.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
        Records {
            attachments: list("attachments")
                .iter()
                .map(|a| Attachment {
                    token: s(a, "token"),
                    plugin: s(a, "plugin"),
                    major: a.get("major").and_then(Value::as_u64).unwrap_or(0),
                    scope: s(a, "scope"),
                    title: s(a, "title"),
                    thread: s(a, "thread"),
                    needs_update: a.get("needs_update").and_then(Value::as_bool).unwrap_or(false),
                })
                .collect(),
            pins: list("pins")
                .iter()
                .map(|p| Pin {
                    plugin: s(p, "plugin"),
                    app: s(p, "app"),
                    ticket: Some(s(p, "ticket")).filter(|t| !t.is_empty()),
                    placement: s(p, "placement"),
                    title: s(p, "title"),
                    needs_update: p.get("needs_update").and_then(Value::as_bool).unwrap_or(false),
                })
                .collect(),
            opened: v
                .get("opened")
                .and_then(Value::as_object)
                .map(|m| m.iter().map(|(k, f)| (k.clone(), f.as_str().unwrap_or_default().to_owned())).collect())
                .unwrap_or_default(),
            migrated_at: v.get("migrated_at").and_then(Value::as_i64).unwrap_or(0),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "attachments": self.attachments.iter().map(|a| json!({
                "token": a.token, "plugin": a.plugin, "major": a.major, "scope": a.scope,
                "title": a.title, "thread": a.thread, "needs_update": a.needs_update,
            })).collect::<Vec<_>>(),
            "pins": self.pins.iter().map(|p| json!({
                "plugin": p.plugin, "app": p.app, "ticket": p.ticket, "placement": p.placement, "title": p.title,
                "needs_update": p.needs_update,
            })).collect::<Vec<_>>(),
            "opened": self.opened.iter().map(|(k, f)| (k.clone(), json!(f))).collect::<serde_json::Map<_, _>>(),
            "migrated_at": self.migrated_at,
        })
    }

    /// Writes the records to `dir` (a new file renamed over the old).
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join("mini-v1.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.to_json()).unwrap_or_default())?;
        std::fs::rename(tmp, file(dir))
    }

    /// Attaches canonical `token` to `thread` (7.6): false when it was
    /// attached there already (the existing one stays).
    pub fn attach(&mut self, thread: &str, token: &str, title: &str, plugin: &PluginEntry) -> bool {
        let Some((_, scope, _)) = tokens::canonical(token) else { return false };
        if self.attachments.iter().any(|a| a.thread == thread && a.token == token) {
            return false;
        }
        self.attachments.push(Attachment {
            token: token.to_owned(),
            plugin: plugin.id.clone(),
            major: major(&plugin.version),
            scope: scope.to_owned(),
            title: title.to_owned(),
            thread: thread.to_owned(),
            needs_update: false,
        });
        true
    }

    /// Detaches `token` from `thread` (the card stays, 10.3).
    pub fn detach(&mut self, thread: &str, token: &str) -> bool {
        let n = self.attachments.len();
        self.attachments.retain(|a| !(a.thread == thread && a.token == token));
        n != self.attachments.len()
    }

    /// Pins `pin`; false when the same pin is there.
    pub fn pin(&mut self, pin: Pin) -> bool {
        let same = |p: &Pin| p.plugin == pin.plugin && p.app == pin.app && p.ticket == pin.ticket;
        if self.pins.iter().any(same) {
            return false;
        }
        self.pins.push(pin);
        true
    }

    /// Takes a pin out.
    pub fn unpin(&mut self, plugin: &str, app: &str, ticket: Option<&str>) {
        self.pins.retain(|p| !(p.plugin == plugin && p.app == app && p.ticket.as_deref() == ticket));
    }

    /// The attachments of `thread`.
    pub fn of_thread(&self, thread: &str) -> Vec<Attachment> {
        self.attachments.iter().filter(|a| a.thread == thread).cloned().collect()
    }

    /// The canonical tokens of `thread` the model may see (10.3, AGT-06):
    /// only attachments whose plugin is installed, enabled and on the
    /// major they were attached with.
    pub fn model_tokens(&self, thread: &str, plugins: &[PluginEntry]) -> Vec<Value> {
        self.attachments
            .iter()
            .filter(|a| a.thread == thread && status(a, plugins) == Status::Ok)
            .map(|a| json!({"token": a.token, "plugin": a.plugin, "scope": a.scope, "title": a.title}))
            .collect()
    }

    /// Records that `plugin`'s `app` opened with `fingerprint` (12.2): the
    /// fingerprint it had before, when it changed.
    pub fn opened(&mut self, plugin: &str, app: &str, fingerprint: &str) -> Option<String> {
        let k = format!("{plugin}/{app}");
        match self.opened.iter_mut().find(|(key, _)| *key == k) {
            Some((_, f)) if f == fingerprint => None,
            Some((_, f)) => Some(std::mem::replace(f, fingerprint.to_owned())),
            None => {
                self.opened.push((k, fingerprint.to_owned()));
                None
            }
        }
    }

    /// Removes everything that names `plugin` (13.6 step 4): its
    /// attachments in every chat, its pins, its apps' fingerprints. How
    /// many attachments and pins went.
    pub fn remove_plugin(&mut self, plugin: &str) -> (usize, usize) {
        let (a, p) = (self.attachments.len(), self.pins.len());
        self.attachments.retain(|x| x.plugin != plugin);
        self.pins.retain(|x| x.plugin != plugin);
        let prefix = format!("{plugin}/");
        self.opened.retain(|(k, _)| !k.starts_with(&prefix));
        (a - self.attachments.len(), p - self.pins.len())
    }
}

/// How `a` stands against `plugins` (10.3, 13.4).
pub fn status(a: &Attachment, plugins: &[PluginEntry]) -> Status {
    match plugins.iter().find(|p| p.id == a.plugin) {
        None => Status::NotInstalled,
        Some(p) if !p.enabled => Status::Disabled(p.title.clone()),
        Some(_) if a.needs_update || a.token.is_empty() => Status::NeedsUpdate,
        Some(p) if major(&p.version) != a.major => Status::NeedsUpdate,
        Some(_) => Status::Ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "tkt_0123456789ABCDEFGHJKMNPQRS";

    pub(crate) fn plugin(id: &str, version: &str, enabled: bool) -> PluginEntry {
        PluginEntry::from_json(&json!({"id": id, "short": "bir", "title": "BIR", "version": version, "enabled": enabled})).unwrap()
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rox-records-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn agt_06_attachments_store_canonical_tokens_and_a_major_change_keeps_them_from_the_model() {
        let mut r = Records::default();
        let v1 = plugin("dev.goldcoders.bir", "1.4.0", true);
        let tok = format!("@bir:taxprofile:{T}");
        assert!(!r.attach("th1", "@bir:taxprofile:123456789", "Jane", &v1), "a typed token is not stored");
        assert!(r.attach("th1", &tok, "Dela Cruz Jane", &v1));
        assert!(!r.attach("th1", &tok, "Dela Cruz Jane", &v1), "attached twice: the existing one");
        let a = &r.attachments[0];
        assert_eq!((a.token.as_str(), a.plugin.as_str(), a.major, a.scope.as_str(), a.title.as_str(), a.thread.as_str()),
            (tok.as_str(), "dev.goldcoders.bir", 1, "taxprofile", "Dela Cruz Jane", "th1"));
        // same major: sent
        assert_eq!(r.model_tokens("th1", &[plugin("dev.goldcoders.bir", "1.9.3", true)]).len(), 1);
        assert!(r.model_tokens("th2", &[v1.clone()]).is_empty());
        // a new major: needs update, kept, not sent
        let v2 = plugin("dev.goldcoders.bir", "2.0.0", true);
        assert_eq!(status(a, &[v2.clone()]), Status::NeedsUpdate);
        assert_eq!(status(a, &[v2.clone()]).label(), "needs update");
        assert!(r.model_tokens("th1", &[v2]).is_empty());
        assert_eq!(r.attachments.len(), 1, "the ticket is kept");
        // disabled or gone: not sent either
        assert_eq!(status(a, &[plugin("dev.goldcoders.bir", "1.4.0", false)]), Status::Disabled("BIR".into()));
        assert_eq!(status(a, &[]), Status::NotInstalled);
        assert!(r.model_tokens("th1", &[]).is_empty());
        // detach removes only the attachment
        assert!(r.detach("th1", &tok));
        assert!(r.attachments.is_empty());
    }

    #[test]
    fn pins_keep_plugin_app_ticket_and_placement() {
        let mut r = Records::default();
        let pin = Pin { plugin: "dev.goldcoders.bir".into(), app: "forms".into(), ticket: Some(T.into()), placement: "full".into(), title: "Forms".into(), needs_update: false };
        assert!(r.pin(pin.clone()));
        assert!(!r.pin(pin.clone()));
        let dir = temp("pins");
        r.save(&dir).unwrap();
        let back = Records::load(&dir);
        assert_eq!(back.pins, [pin]);
        assert!(!back.to_json().to_string().contains("fingerprint"));
        r.unpin("dev.goldcoders.bir", "forms", Some(T));
        assert!(r.pins.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lcy_04_the_last_opened_fingerprint_says_when_an_app_changed() {
        let mut r = Records::default();
        assert_eq!(r.opened("dev.goldcoders.bir", "forms", "sha256:aa"), None);
        assert_eq!(r.opened("dev.goldcoders.bir", "forms", "sha256:aa"), None);
        assert_eq!(r.opened("dev.goldcoders.bir", "forms", "sha256:bb").as_deref(), Some("sha256:aa"));
        assert_eq!(r.opened("dev.goldcoders.bir", "calendar", "sha256:bb"), None);
    }

    #[test]
    fn lcy_06_uninstall_removes_the_plugins_attachments_pins_and_fingerprints() {
        let mut r = Records::default();
        let bir = plugin("dev.goldcoders.bir", "1.0.0", true);
        let other = PluginEntry { id: "dev.goldcoders.other".into(), short: "oth".into(), ..bir.clone() };
        r.attach("th1", &format!("@bir:taxprofile:{T}"), "Jane", &bir);
        r.attach("th2", &format!("@bir:branchcode:{T}"), "Branch", &bir);
        r.attach("th1", &format!("@oth:item:{T}"), "Item", &other);
        r.pin(Pin { plugin: bir.id.clone(), app: "forms".into(), ..Default::default() });
        r.pin(Pin { plugin: other.id.clone(), app: "board".into(), ..Default::default() });
        r.opened(&bir.id, "forms", "sha256:aa");
        assert_eq!(r.remove_plugin(&bir.id), (2, 1));
        assert!(r.attachments.iter().all(|a| a.plugin == other.id));
        assert!(r.pins.iter().all(|p| p.plugin == other.id));
        assert!(r.opened.is_empty());
    }

    #[test]
    fn records_round_trip_and_a_broken_file_starts_empty() {
        let dir = temp("round");
        let mut r = Records { migrated_at: 5, ..Default::default() };
        r.attach("th", &format!("@bir:taxprofile:{T}"), "Jane", &plugin("dev.goldcoders.bir", "1.0.0", true));
        r.opened("p.q.r", "a", "sha256:x");
        r.save(&dir).unwrap();
        assert_eq!(Records::load(&dir), r);
        std::fs::write(file(&dir), "{oops").unwrap();
        assert_eq!(Records::load(&dir), Records::default());
        assert_eq!(major("2.1.0"), 2);
        assert_eq!(major("1.0.0-rc.1"), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
