//! A plugin's badge on its pins (16.4, `_meta["buwiz/badge"]`).
//!
//! Any tool result may carry one badge: `{app, dot?, text?, ticket?}` to set
//! it, `{app, clear: true, ticket?}` to take it away. [`Host::note_badge`] is
//! the one reader: every call to a plugin's tool (an app's signed call, the
//! model's, a routine's query, a widget's, a resolve) goes through
//! [`Host::call_tool`], which hands each result to it. A result stays as the
//! plugin made it; the callers strip `_meta` as they do for every key.
//!
//! The host keeps one badge per (plugin, app, ticket or none) in `v1_badges`
//! and says each change on [`Host::badge_events`]; the plugin list carries
//! them (`badges`), so an app that starts later shows them too. A badge is
//! changed on the event stream at most once a second per key: a faster
//! plugin's changes are collapsed to the latest value, which the store has
//! at once.

use std::time::{Duration, Instant};

use rusqlite::params;
use serde_json::{json, Value};

use super::Host;
use crate::helper::sessions::log_once;
use crate::js;
use crate::v1::{get, is_v1};

pub const KEY: &str = "buwiz/badge";
/// The most characters a badge's text has.
pub const TEXT_MAX: usize = 8;
/// The dots a badge may show; `none` shows no dot.
pub const DOTS: [&str; 4] = ["red", "amber", "green", "none"];
/// The shortest time between two events about one badge.
const GAP: Duration = Duration::from_secs(1);

pub const SCHEMA: &str = "
  CREATE TABLE IF NOT EXISTS v1_badges(
    plugin TEXT NOT NULL, app TEXT NOT NULL, ticket TEXT NOT NULL DEFAULT '',
    dot TEXT NOT NULL DEFAULT 'none', text TEXT NOT NULL DEFAULT '', updated_at INTEGER NOT NULL,
    PRIMARY KEY(plugin, app, ticket));
";

/// What a result's badge asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub app: String,
    pub ticket: Option<String>,
    /// `None`: a clear.
    pub set: Option<(String, String)>,
}

/// `v`, the value of a result's `buwiz/badge`, judged against the plugin's own
/// manifest `m`. The reason, when it is malformed.
pub fn parse(m: &Value, v: &Value) -> Result<Parsed, &'static str> {
    let o = v.as_object().ok_or("it is not an object")?;
    let app = o.get("app").and_then(Value::as_str).ok_or("app is not a string")?;
    if !get(m, "apps").and_then(Value::as_array).into_iter().flatten().any(|a| get(a, "id").and_then(Value::as_str) == Some(app)) {
        return Err("app is not an app of this plugin");
    }
    let ticket = match o.get("ticket") {
        None => None,
        Some(t) if t.is_string() && is_v1("ticket", Some(t)) => t.as_str().map(str::to_owned),
        Some(_) => return Err("ticket breaks the ticket grammar"),
    };
    let ok = |set| Ok(Parsed { app: app.to_owned(), ticket: ticket.clone(), set });
    match o.get("clear") {
        None => {}
        Some(Value::Bool(true)) if !o.contains_key("dot") && !o.contains_key("text") => return ok(None),
        Some(Value::Bool(true)) => return Err("a clear carries only app and ticket"),
        Some(_) => return Err("clear is not true"),
    }
    let dot = match o.get("dot") {
        None => None,
        Some(d) => Some(d.as_str().filter(|d| DOTS.contains(d)).ok_or("dot is not red, amber, green or none")?),
    };
    let text = match o.get("text") {
        None => None,
        Some(t) => Some(one_line(t.as_str().ok_or("text is not a string")?)?),
    };
    if dot.is_none() && text.is_none() {
        return Err("it sets neither dot nor text");
    }
    ok(Some((dot.unwrap_or("none").to_owned(), text.unwrap_or_default())))
}

/// `t` trimmed: at most [`TEXT_MAX`] characters, one line.
fn one_line(t: &str) -> Result<String, &'static str> {
    let t = t.trim();
    if t.chars().any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}') {
        return Err("text is not one line of plain characters");
    }
    if t.chars().count() > TEXT_MAX {
        return Err("text is over 8 characters");
    }
    Ok(t.to_owned())
}

/// A badge as the list and the event carry it; a cleared one has dot `none`
/// and no text.
fn view(plugin: Option<&str>, app: &str, ticket: &str, dot: &str, text: &str) -> Value {
    let mut v = json!({"app": app, "ticket": (!ticket.is_empty()).then_some(ticket), "dot": dot, "text": text});
    if let Some(p) = plugin {
        v["plugin"] = json!(p);
    }
    v
}

/// Per badge: when it last went out, and whether a later one waits.
#[derive(Default)]
pub struct Gate {
    last: Option<Instant>,
    waiting: bool,
}

impl Host {
    /// The one reader of `_meta["buwiz/badge"]`: `r` is a result of plugin
    /// `id`'s tool. Ignored on a failed result, for a plugin that is not
    /// enabled and approved; a malformed badge is dropped and logged once per
    /// plugin and reason.
    pub(crate) fn note_badge(&self, id: &str, r: &Value) {
        let Some(b) = r.get("_meta").and_then(|m| m.get(KEY)) else { return };
        if r.get("isError").is_some_and(js::truthy) {
            return;
        }
        let Some(row) = self.row(id) else { return };
        if !row.enabled || !self.trust().is_approved(&row.id, &row.publisher_key_id) {
            return;
        }
        match parse(&row.manifest(), b) {
            Ok(p) => self.set_badge(id, &p),
            Err(why) => log_once(format!("badge\0{id}\0{why}"), || format!("  v1 {id}: buwiz/badge dropped: {why}")),
        }
    }

    fn set_badge(&self, id: &str, p: &Parsed) {
        let ticket = p.ticket.clone().unwrap_or_default();
        let now = self.clock.now() as i64;
        let shown = p.set.clone().filter(|(dot, text)| dot != "none" || !text.is_empty());
        let changed = self.sql(|db| match &shown {
            None => Ok(db.execute("DELETE FROM v1_badges WHERE plugin=? AND app=? AND ticket=?", params![id, p.app, ticket])? > 0),
            Some((dot, text)) => {
                let same: bool = db.query_row("SELECT count(*) FROM v1_badges WHERE plugin=? AND app=? AND ticket=? AND dot=? AND text=?", params![id, p.app, ticket, dot, text], |r| r.get::<_, i64>(0))? > 0;
                if !same {
                    db.execute("INSERT OR REPLACE INTO v1_badges(plugin,app,ticket,dot,text,updated_at) VALUES(?,?,?,?,?,?)", params![id, p.app, ticket, dot, text, now])?;
                }
                Ok(!same)
            }
        });
        if changed.unwrap_or(false) {
            self.announce_badge(id, &p.app, &ticket);
        }
    }

    /// The events of badge changes (`event: badge` on `/v1/host/events`).
    pub fn badge_events(&self) -> tokio::sync::broadcast::Receiver<Value> {
        self.badge_events.subscribe()
    }

    /// The badges of plugin `id`, for its entry in the plugin list.
    pub fn badges_view(&self, id: &str) -> Value {
        let rows = self.sql(|db| {
            let mut st = db.prepare("SELECT app, ticket, dot, text FROM v1_badges WHERE plugin=? ORDER BY app, ticket")?;
            let rows = st.query_map([id], |r| Ok(view(None, &r.get::<_, String>(0)?, &r.get::<_, String>(1)?, &r.get::<_, String>(2)?, &r.get::<_, String>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>();
            rows
        });
        json!(rows.unwrap_or_default())
    }

    /// Says what badge (`app`, `ticket`) of `id` is now, at once when it has
    /// been quiet for a second, else once the second is over, then with the
    /// latest value.
    fn announce_badge(&self, id: &str, app: &str, ticket: &str) {
        let key = (id.to_owned(), app.to_owned(), ticket.to_owned());
        let now = Instant::now();
        let wait = {
            let mut gates = self.badge_gates.lock().unwrap_or_else(|e| e.into_inner());
            let g = gates.entry(key.clone()).or_default();
            match g.last.map(|t| GAP.saturating_sub(now.duration_since(t))).filter(|d| !d.is_zero()) {
                None => {
                    g.last = Some(now);
                    None
                }
                Some(_) if g.waiting => return,
                Some(d) => {
                    g.waiting = true;
                    Some(d)
                }
            }
        };
        let Some(wait) = wait else { return self.emit_badge(id, app, ticket) };
        let me = self.me.clone();
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            let Some(host) = me.upgrade() else { return };
            {
                let mut gates = host.badge_gates.lock().unwrap_or_else(|e| e.into_inner());
                let g = gates.entry(key.clone()).or_default();
                g.waiting = false;
                g.last = Some(Instant::now());
            }
            host.emit_badge(&key.0, &key.1, &key.2);
        });
    }

    /// The event of badge (`app`, `ticket`) of `id` as it is stored now.
    fn emit_badge(&self, id: &str, app: &str, ticket: &str) {
        let now = self.sql(|db| {
            db.query_row("SELECT dot, text FROM v1_badges WHERE plugin=? AND app=? AND ticket=?", params![id, app, ticket], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map(Some)
                .or_else(|e| if matches!(e, rusqlite::Error::QueryReturnedNoRows) { Ok(None) } else { Err(e) })
        });
        let (dot, text) = now.ok().flatten().unwrap_or_else(|| ("none".to_owned(), String::new()));
        let _ = self.badge_events.send(view(Some(id), app, ticket, &dot, &text));
    }

    /// Removes the badges of `id`, all, or those of apps not in `keep`
    /// (an update's manifest), and says each is gone.
    pub(crate) fn drop_badges(&self, id: &str, keep: Option<&[String]>) {
        let gone: Vec<(String, String)> = self
            .sql(|db| {
                let mut st = db.prepare("SELECT app, ticket FROM v1_badges WHERE plugin=?")?;
                let rows = st.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>();
                rows
            })
            .unwrap_or_default()
            .into_iter()
            .filter(|(app, _)| keep.is_none_or(|k| !k.contains(app)))
            .collect();
        for (app, ticket) in gone {
            let _ = self.sql(|db| db.execute("DELETE FROM v1_badges WHERE plugin=? AND app=? AND ticket=?", params![id, app, ticket]));
            let _ = self.badge_events.send(view(Some(id), &app, &ticket, "none", ""));
        }
        if keep.is_none() {
            self.badge_gates.lock().unwrap_or_else(|e| e.into_inner()).retain(|k, _| k.0 != id);
        }
    }
}

#[cfg(test)]
#[path = "badges_tests.rs"]
mod tests;
