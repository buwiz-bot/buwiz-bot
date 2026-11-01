//! What Rox keeps for v1 plugins in its own data dir
//! (`<data dir>/mini-v1.json`): the chats' attachments (10.3), the pins
//! (12.3), the fingerprint each app was last opened with (12.2), the
//! version each plugin was last seen at (its Connect Apps page says
//! "Updated to v…" for a day after it changes) and whether the one-time
//! v0 migration ran (Q11).
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

/// A message sent with v1 attachments (10.3): what the model got with it,
/// shown on the person's bubble.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sent {
    pub thread: String,
    pub text: String,
    /// (canonical token, title)
    pub tokens: Vec<(String, String)>,
}

/// How many sent messages are remembered.
const MAX_SENT: usize = 500;

/// How long a plugin's page says it was updated (ms): a day.
pub const UPDATED_FOR_MS: i64 = 24 * 60 * 60 * 1000;

/// The version a plugin was last seen at, and when it changed to it (ms;
/// 0 when it has not changed since Rox first saw the plugin).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Seen {
    pub plugin: String,
    pub version: String,
    pub changed_at: i64,
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
    /// The last messages sent with attachments.
    pub sent: Vec<Sent>,
    /// Each plugin's version as last listed.
    pub versions: Vec<Seen>,
}

/// The file in `dir` (Rox's data dir).
pub fn file(dir: &Path) -> PathBuf {
    dir.join("mini-v1.json")
}

/// The rox names a stored record may hold, and their buwiz ones
/// (protocol 2.0; the broker's app.db gets the same rewrite).
const RENAMES: [(&str, &str); 7] = [
    ("rox-app://", "buwiz-app://"),
    ("rox://", "buwiz://"),
    ("\"rox-app\"", "\"buwiz-app\""),
    ("\"rox/open\"", "\"buwiz/open\""),
    ("\"rox/suggest\"", "\"buwiz/suggest\""),
    ("\"rox/audience\"", "\"buwiz/audience\""),
    ("\"rox/caller\"", "\"buwiz/caller\""),
];

/// `text` with every rox name of [`RENAMES`] spelled the buwiz way.
pub fn buwiz_names(text: &str) -> String {
    RENAMES.iter().fold(text.to_owned(), |t, (a, b)| t.replace(a, b))
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned()
}

impl Records {
    /// The records in `dir`; none yet (or unreadable) is empty.
    pub fn load(dir: &Path) -> Records {
        let Ok(text) = std::fs::read_to_string(file(dir)) else { return Records::default() };
        // protocol 2.0: a file from before it names links and providers
        // the rox way; rewritten once, and saved with the marker
        let marked = serde_json::from_str::<Value>(&text).is_ok_and(|v| v.get("names").and_then(Value::as_str) == Some("buwiz"));
        let renamed = buwiz_names(&text);
        if !marked && serde_json::from_str::<Value>(&renamed).is_ok() {
            let records = Records::parse(&renamed, dir);
            if let Err(e) = records.save(dir) {
                log::warn!("mini v1: could not rewrite {} with the buwiz names: {e}", file(dir).display());
            }
            return records;
        }
        Records::parse(&text, dir)
    }

    fn parse(text: &str, dir: &Path) -> Records {
        let Ok(v) = serde_json::from_str::<Value>(text) else {
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
            sent: list("sent")
                .iter()
                .map(|m| Sent {
                    thread: s(m, "thread"),
                    text: s(m, "text"),
                    tokens: m
                        .get("tokens")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|t| (s(t, "token"), s(t, "title")))
                        .filter(|(t, _)| tokens::canonical(t).is_some())
                        .collect(),
                })
                .collect(),
            versions: v
                .get("versions")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .map(|(k, x)| Seen {
                            plugin: k.clone(),
                            version: s(x, "version"),
                            changed_at: x.get("changed_at").and_then(Value::as_i64).unwrap_or(0),
                        })
                        .collect()
                })
                .unwrap_or_default(),
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
            "names": "buwiz",
            "sent": self.sent.iter().map(|m| json!({
                "thread": m.thread, "text": m.text,
                "tokens": m.tokens.iter().map(|(t, ti)| json!({"token": t, "title": ti})).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "versions": self.versions.iter().map(|x| (x.plugin.clone(), json!({
                "version": x.version, "changed_at": x.changed_at,
            }))).collect::<serde_json::Map<_, _>>(),
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

    /// Remembers that `text` went to `thread` with `tokens` (oldest
    /// forgotten past the cap).
    pub fn sent(&mut self, thread: &str, text: &str, tokens: Vec<(String, String)>) {
        if tokens.is_empty() {
            return;
        }
        self.sent.push(Sent { thread: thread.to_owned(), text: text.trim().to_owned(), tokens });
        if self.sent.len() > MAX_SENT {
            self.sent.remove(0);
        }
    }

    /// What message `text` of `thread` went to the model with (the last
    /// time it was sent).
    pub fn sent_with(&self, thread: &str, text: &str) -> Vec<(String, String)> {
        let text = text.trim();
        self.sent.iter().rev().find(|m| m.thread == thread && m.text == text).map(|m| m.tokens.clone()).unwrap_or_default()
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

    /// Records the `(plugin, version)`s the broker lists now, at `now`
    /// (ms): a plugin seen for the first time is not an update; one whose
    /// version differs from the last seen changed at `now`. Whether
    /// anything is new (the records want saving).
    pub fn saw_versions(&mut self, listed: &[(String, String)], now: i64) -> bool {
        let mut new = false;
        for (plugin, version) in listed.iter().filter(|(_, v)| !v.is_empty()) {
            match self.versions.iter_mut().find(|x| x.plugin == *plugin) {
                Some(x) if x.version == *version => {}
                Some(x) => {
                    x.version = version.clone();
                    x.changed_at = now;
                    new = true;
                }
                None => {
                    self.versions.push(Seen { plugin: plugin.clone(), version: version.clone(), changed_at: 0 });
                    new = true;
                }
            }
        }
        new
    }

    /// What `plugin`'s page says at `now` (ms): "Updated to v1.2.3" for a
    /// day after its version changed, else nothing.
    pub fn updated(&self, plugin: &str, now: i64) -> Option<String> {
        let x = self.versions.iter().find(|x| x.plugin == plugin)?;
        let fresh = x.changed_at > 0 && now >= x.changed_at && now - x.changed_at < UPDATED_FOR_MS;
        fresh.then(|| format!("Updated to v{}", x.version.trim_start_matches('v')))
    }

    /// Removes everything that names `plugin` (13.6 step 4): its
    /// attachments in every chat, its pins, its apps' fingerprints, the
    /// version it was seen at. How many attachments and pins went.
    pub fn remove_plugin(&mut self, plugin: &str) -> (usize, usize) {
        let (a, p) = (self.attachments.len(), self.pins.len());
        self.attachments.retain(|x| x.plugin != plugin);
        self.pins.retain(|x| x.plugin != plugin);
        let prefix = format!("{plugin}/");
        self.opened.retain(|(k, _)| !k.starts_with(&prefix));
        self.versions.retain(|x| x.plugin != plugin);
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
        let v1 = plugin("app.buwiz.ebirforms", "1.4.0", true);
        let tok = format!("@bir:taxprofile:{T}");
        assert!(!r.attach("th1", "@bir:taxprofile:123456789", "Jane", &v1), "a typed token is not stored");
        assert!(r.attach("th1", &tok, "Dela Cruz Jane", &v1));
        assert!(!r.attach("th1", &tok, "Dela Cruz Jane", &v1), "attached twice: the existing one");
        let a = &r.attachments[0];
        assert_eq!((a.token.as_str(), a.plugin.as_str(), a.major, a.scope.as_str(), a.title.as_str(), a.thread.as_str()),
            (tok.as_str(), "app.buwiz.ebirforms", 1, "taxprofile", "Dela Cruz Jane", "th1"));
        // same major: sent
        assert_eq!(r.model_tokens("th1", &[plugin("app.buwiz.ebirforms", "1.9.3", true)]).len(), 1);
        assert!(r.model_tokens("th2", std::slice::from_ref(&v1)).is_empty());
        // a new major: needs update, kept, not sent
        let v2 = plugin("app.buwiz.ebirforms", "2.0.0", true);
        assert_eq!(status(a, std::slice::from_ref(&v2)), Status::NeedsUpdate);
        assert_eq!(status(a, std::slice::from_ref(&v2)).label(), "needs update");
        assert!(r.model_tokens("th1", &[v2]).is_empty());
        assert_eq!(r.attachments.len(), 1, "the ticket is kept");
        // disabled or gone: not sent either
        assert_eq!(status(a, &[plugin("app.buwiz.ebirforms", "1.4.0", false)]), Status::Disabled("BIR".into()));
        assert_eq!(status(a, &[]), Status::NotInstalled);
        assert!(r.model_tokens("th1", &[]).is_empty());
        // detach removes only the attachment
        assert!(r.detach("th1", &tok));
        assert!(r.attachments.is_empty());
    }

    #[test]
    fn pins_keep_plugin_app_ticket_and_placement() {
        let mut r = Records::default();
        let pin = Pin { plugin: "app.buwiz.ebirforms".into(), app: "forms".into(), ticket: Some(T.into()), placement: "full".into(), title: "Forms".into(), needs_update: false };
        assert!(r.pin(pin.clone()));
        assert!(!r.pin(pin.clone()));
        let dir = temp("pins");
        r.save(&dir).unwrap();
        let back = Records::load(&dir);
        assert_eq!(back.pins, [pin]);
        assert!(!back.to_json().to_string().contains("fingerprint"));
        r.unpin("app.buwiz.ebirforms", "forms", Some(T));
        assert!(r.pins.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lcy_04_the_last_opened_fingerprint_says_when_an_app_changed() {
        let mut r = Records::default();
        assert_eq!(r.opened("app.buwiz.ebirforms", "forms", "sha256:aa"), None);
        assert_eq!(r.opened("app.buwiz.ebirforms", "forms", "sha256:aa"), None);
        assert_eq!(r.opened("app.buwiz.ebirforms", "forms", "sha256:bb").as_deref(), Some("sha256:aa"));
        assert_eq!(r.opened("app.buwiz.ebirforms", "calendar", "sha256:bb"), None);
    }

    #[test]
    fn lcy_06_uninstall_removes_the_plugins_attachments_pins_and_fingerprints() {
        let mut r = Records::default();
        let bir = plugin("app.buwiz.ebirforms", "1.0.0", true);
        let other = PluginEntry { id: "app.buwiz.other".into(), short: "oth".into(), ..bir.clone() };
        r.attach("th1", &format!("@bir:taxprofile:{T}"), "Jane", &bir);
        r.attach("th2", &format!("@bir:branchcode:{T}"), "Branch", &bir);
        r.attach("th1", &format!("@oth:item:{T}"), "Item", &other);
        r.pin(Pin { plugin: bir.id.clone(), app: "forms".into(), ..Default::default() });
        r.pin(Pin { plugin: other.id.clone(), app: "board".into(), ..Default::default() });
        r.opened(&bir.id, "forms", "sha256:aa");
        r.saw_versions(&[(bir.id.clone(), "1.0.0".into())], 1);
        assert_eq!(r.remove_plugin(&bir.id), (2, 1));
        assert!(r.attachments.iter().all(|a| a.plugin == other.id));
        assert!(r.pins.iter().all(|p| p.plugin == other.id));
        assert!(r.opened.is_empty());
        assert!(r.versions.is_empty(), "a reinstall is a first sight, not an update");
    }

    #[test]
    fn a_plugins_page_says_it_was_updated_for_a_day_after_its_version_changes() {
        const BIR: &str = "app.buwiz.ebirforms";
        let at = |v: &str| vec![(BIR.to_owned(), v.to_owned())];
        let t0 = 1_800_000_000_000;
        let mut r = Records::default();
        // first sight: remembered, not an update
        assert!(r.saw_versions(&at("1.0.0"), t0));
        assert_eq!(r.updated(BIR, t0), None);
        // the same version again: nothing new, nothing to say
        assert!(!r.saw_versions(&at("1.0.0"), t0 + 1000));
        assert!(!r.saw_versions(&at(""), t0 + 1000), "a listing without a version is skipped");
        assert_eq!(r.updated(BIR, t0 + 1000), None);
        // a change: said from then on
        let t1 = t0 + 5000;
        assert!(r.saw_versions(&at("1.2.3"), t1));
        assert_eq!(r.updated(BIR, t1).as_deref(), Some("Updated to v1.2.3"));
        assert!(!r.saw_versions(&at("1.2.3"), t1 + 60_000), "listing it again keeps the time it changed");
        assert_eq!(r.updated(BIR, t1 + UPDATED_FOR_MS - 1).as_deref(), Some("Updated to v1.2.3"));
        assert_eq!(r.updated("app.buwiz.other", t1), None);
        // a restart keeps it: neither lost nor announced again
        let dir = temp("versions");
        r.save(&dir).unwrap();
        let mut back = Records::load(&dir);
        assert_eq!(back.versions, r.versions);
        assert!(!back.saw_versions(&at("1.2.3"), t1 + 120_000));
        assert_eq!(back.updated(BIR, t1 + 120_000).as_deref(), Some("Updated to v1.2.3"));
        // 24 hours on: gone
        assert_eq!(back.updated(BIR, t1 + UPDATED_FOR_MS), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_from_before_protocol_2_gets_the_buwiz_names_once() {
        let dir = temp("names");
        std::fs::create_dir_all(&dir).unwrap();
        let old = r#"{"attachments": [], "pins": [], "opened": {}, "migrated_at": 7,
          "sent": [{"thread": "th1", "text": "see rox://app/app.buwiz.ebirforms/forms and rox://chat/t1?m=2", "tokens": []}]}"#;
        std::fs::write(file(&dir), old).unwrap();
        let r = Records::load(&dir);
        assert_eq!(r.sent[0].text, "see buwiz://app/app.buwiz.ebirforms/forms and buwiz://chat/t1?m=2");
        assert_eq!(r.migrated_at, 7);
        let on_disk = std::fs::read_to_string(file(&dir)).unwrap();
        assert!(on_disk.contains(r#""names": "buwiz""#) && !on_disk.contains("rox://"), "{on_disk}");
        // marked: a later load changes nothing, even text that looks old
        let mut v: Value = serde_json::from_str(&on_disk).unwrap();
        v["sent"][0]["text"] = json!("rox://kept as typed");
        std::fs::write(file(&dir), v.to_string()).unwrap();
        assert_eq!(Records::load(&dir).sent[0].text, "rox://kept as typed");
        assert_eq!(buwiz_names(r#"{"provider":"rox-app","entry":"rox-app://card","_meta":{"rox/caller":1}}"#), r#"{"provider":"buwiz-app","entry":"buwiz-app://card","_meta":{"buwiz/caller":1}}"#);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sent_message_remembers_what_the_model_got() {
        let mut r = Records::default();
        let tok = format!("@bir:taxprofile:{T}");
        r.sent("th1", "what are the due forms ", vec![(tok.clone(), "Dela Cruz Jane".into())]);
        r.sent("th1", "no attachments", vec![]);
        assert_eq!(r.sent_with("th1", "what are the due forms"), [(tok.clone(), "Dela Cruz Jane".to_owned())]);
        assert!(r.sent_with("th2", "what are the due forms").is_empty());
        assert!(r.sent_with("th1", "no attachments").is_empty());
        let dir = temp("sent");
        r.save(&dir).unwrap();
        assert_eq!(Records::load(&dir).sent, r.sent);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn records_round_trip_and_a_broken_file_starts_empty() {
        let dir = temp("round");
        let mut r = Records { migrated_at: 5, ..Default::default() };
        r.attach("th", &format!("@bir:taxprofile:{T}"), "Jane", &plugin("app.buwiz.ebirforms", "1.0.0", true));
        r.opened("p.q.r", "a", "sha256:x");
        r.saw_versions(&[("p.q.r".into(), "1.0.0".into())], 5);
        r.saw_versions(&[("p.q.r".into(), "1.1.0".into())], 9);
        r.save(&dir).unwrap();
        assert_eq!(Records::load(&dir), r);
        std::fs::write(file(&dir), "{oops").unwrap();
        assert_eq!(Records::load(&dir), Records::default());
        assert_eq!(major("2.1.0"), 2);
        assert_eq!(major("1.0.0-rc.1"), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
