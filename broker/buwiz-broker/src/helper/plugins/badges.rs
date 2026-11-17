//! A plugin's badge on its pins (16.4, `_meta["buwiz/badge"]`).
//!
//! The result of any call to one of a plugin's own tools may carry one badge:
//! `{app, dot?, text?, ticket?}` to set it, `{app, clear: true, ticket?}` to
//! take it away. [`Host::note_badge`] is the one reader: every call to a
//! plugin's tool (an app's signed call, the model's, a routine's query, a
//! widget's, a resolve tool) goes through [`Host::call_tool`], which hands each
//! result to it. The reserved `buwiz.*` calls do not come this way and are
//! not read for a badge. A result stays as the plugin made it; the callers
//! strip `_meta` as they do for every key.
//!
//! The host keeps one badge per (plugin, app, ticket or none) in `v1_badges`
//! and says each change on [`Host::badge_events`]; the plugin list carries
//! them (`badges`, with `badges_rev`), so an app that starts later shows them
//! too. A plugin holds at most [`CAP`] badges. A badge is changed on the event
//! stream at most once a second per key: a faster plugin's changes are
//! collapsed to the latest value, which the store has at once.
//!
//! Every change has a `rev`, a number that only goes up in this broker (and
//! across its restarts: it follows the clock): each badge in the list and
//! each event carries its own, a removal's too, and the list carries the
//! highest `rev` issued when it was made (`badges_rev`), so an app can tell
//! an event from an older list and a list from an older event.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use rusqlite::params;
use serde_json::{json, Value};

use super::Host;
use crate::helper::sessions::log_once;
use crate::js;
use crate::v1::is_v1;

pub const KEY: &str = "buwiz/badge";
/// The most characters a badge's text has.
pub const TEXT_MAX: usize = 8;
/// The most badges one plugin holds, ticketed and app-wide together.
pub const CAP: usize = 32;
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

/// `v`, the value of a result's `buwiz/badge`, judged against the ids of the
/// plugin's own apps. The reason, when it is malformed. The text is taken as
/// written: nothing is trimmed (`vectors/badge.json` is the shared test of it).
pub fn parse(apps: &[String], v: &Value) -> Result<Parsed, &'static str> {
    let o = v.as_object().ok_or("it is not an object")?;
    let app = o.get("app").and_then(Value::as_str).ok_or("app is not a string")?;
    if !apps.iter().any(|a| a == app) {
        return Err("app is not an app of this plugin");
    }
    let ticket = match o.get("ticket") {
        None | Some(Value::Null) => None,
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
        None => "none",
        Some(d) => d.as_str().filter(|d| DOTS.contains(d)).ok_or("dot is not red, amber, green or none")?,
    };
    let text = match o.get("text") {
        None => None,
        Some(t) => Some(one_line(t.as_str().ok_or("text is not a string")?)?),
    };
    if dot == "none" && text.is_none() {
        return Err("it sets nothing: a badge shows a dot or text, and a clear is clear: true");
    }
    ok(Some((dot.to_owned(), text.unwrap_or_default().to_owned())))
}

/// `t` as written: 1 to [`TEXT_MAX`] characters, no hidden character (spec 4.2), no space at either end.
fn one_line(t: &str) -> Result<&str, &'static str> {
    if t.is_empty() {
        return Err("text is empty");
    }
    if t.chars().any(buwiz_protocol::text::hidden) {
        return Err("text has a hidden character");
    }
    if t.starts_with(' ') || t.ends_with(' ') {
        return Err("text starts or ends with a space");
    }
    if t.chars().count() > TEXT_MAX {
        return Err("text is over 8 characters");
    }
    Ok(t)
}

/// A badge as the list and the event carry it; a cleared one has dot `none`
/// and no text. `rev` orders it against every other change.
fn view(plugin: Option<&str>, app: &str, ticket: &str, dot: &str, text: &str, rev: i64) -> Value {
    let mut v = json!({"app": app, "ticket": (!ticket.is_empty()).then_some(ticket), "dot": dot, "text": text, "rev": rev});
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

/// What a set came to.
enum Done {
    Same,
    Changed,
    Capped,
}

impl Host {
    /// A number higher than any this broker has issued: the clock's, or one more
    /// than the last.
    fn next_rev(&self) -> i64 {
        let now = self.clock.now() as i64;
        let mut last = self.last_rev.load(Ordering::SeqCst);
        loop {
            let next = now.max(last + 1);
            match self.last_rev.compare_exchange(last, next, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return next,
                Err(seen) => last = seen,
            }
        }
    }

    /// The one reader of `_meta["buwiz/badge"]`: `r` is the result of a call to
    /// one of plugin `id`'s own tools (the reserved `buwiz.*` calls are not read).
    /// Ignored on a failed result and for a plugin that is not running, or was not
    /// approved when it was started: what the host held then is used, not the
    /// trust file or the record. A malformed badge, or one past the plugin's
    /// [`CAP`], is dropped and logged once per plugin and reason.
    pub(crate) fn note_badge(&self, id: &str, r: &Value) {
        let Some(b) = r.get("_meta").and_then(|m| m.get(KEY)) else { return };
        if r.get("isError").is_some_and(js::truthy) {
            return;
        }
        let Some((true, apps)) = self.with_rt(id, |rt| (rt.state == "running").then(|| (rt.approved, rt.apps.clone()))) else { return };
        match parse(&apps, b) {
            Ok(p) => self.set_badge(id, &p),
            Err(why) => log_once(format!("badge\0{id}\0{why}"), || format!("  v1 {id}: buwiz/badge dropped: {why}")),
        }
    }

    fn set_badge(&self, id: &str, p: &Parsed) {
        let ticket = p.ticket.clone().unwrap_or_default();
        let shown = p.set.clone();
        let done = self.sql(|db| match &shown {
            None => Ok(if db.execute("DELETE FROM v1_badges WHERE plugin=? AND app=? AND ticket=?", params![id, p.app, ticket])? > 0 { Done::Changed } else { Done::Same }),
            Some((dot, text)) => {
                let same: bool = db.query_row("SELECT count(*) FROM v1_badges WHERE plugin=? AND app=? AND ticket=? AND dot=? AND text=?", params![id, p.app, ticket, dot, text], |r| r.get::<_, i64>(0))? > 0;
                if same {
                    return Ok(Done::Same);
                }
                let held: bool = db.query_row("SELECT count(*) FROM v1_badges WHERE plugin=? AND app=? AND ticket=?", params![id, p.app, ticket], |r| r.get::<_, i64>(0))? > 0;
                let n: i64 = db.query_row("SELECT count(*) FROM v1_badges WHERE plugin=?", [id], |r| r.get(0))?;
                if !held && n as usize >= CAP {
                    return Ok(Done::Capped);
                }
                // the number is taken under the database's lock, so a list made after it has this badge
                db.execute("INSERT OR REPLACE INTO v1_badges(plugin,app,ticket,dot,text,updated_at) VALUES(?,?,?,?,?,?)", params![id, p.app, ticket, dot, text, self.next_rev()])?;
                Ok(Done::Changed)
            }
        });
        match done {
            Ok(Done::Changed) => self.announce_badge(id, &p.app, &ticket),
            Ok(Done::Capped) => log_once(format!("badge\0{id}\0cap"), || format!("  v1 {id}: buwiz/badge dropped: the plugin holds {CAP} badges already")),
            _ => {}
        }
    }

    /// The events of badge changes (`event: badge` on `/v1/host/events`).
    pub fn badge_events(&self) -> tokio::sync::broadcast::Receiver<Value> {
        self.badge_events.subscribe()
    }

    /// The badges of plugin `id` for its entry in the plugin list, with the
    /// highest `rev` issued when they were read: every change up to it is in the
    /// list or was a removal.
    pub fn badges_view(&self, id: &str) -> (i64, Value) {
        let got = self.sql(|db| {
            let at = self.last_rev.load(Ordering::SeqCst);
            let mut st = db.prepare("SELECT app, ticket, dot, text, updated_at FROM v1_badges WHERE plugin=? ORDER BY app, ticket")?;
            let rows = st
                .query_map([id], |r| Ok(view(None, &r.get::<_, String>(0)?, &r.get::<_, String>(1)?, &r.get::<_, String>(2)?, &r.get::<_, String>(3)?, r.get(4)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((at, rows))
        });
        let (at, rows) = got.unwrap_or_default();
        (at, json!(rows))
    }

    /// Every plugin's badges from one read: the highest `rev` issued when it was made, and each
    /// plugin's list (as [`Host::badges_view`] has one plugin's; a plugin not in it has none).
    pub fn badges_all(&self) -> (i64, std::collections::HashMap<String, Value>) {
        let got = self.sql(|db| {
            let at = self.last_rev.load(Ordering::SeqCst);
            let mut st = db.prepare("SELECT plugin, app, ticket, dot, text, updated_at FROM v1_badges ORDER BY plugin, app, ticket")?;
            let rows = st
                .query_map([], |r| Ok((r.get::<_, String>(0)?, view(None, &r.get::<_, String>(1)?, &r.get::<_, String>(2)?, &r.get::<_, String>(3)?, &r.get::<_, String>(4)?, r.get(5)?))))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((at, rows))
        });
        let (at, rows) = got.unwrap_or_default();
        let mut out: std::collections::HashMap<String, Vec<Value>> = std::collections::HashMap::new();
        for (plugin, b) in rows {
            out.entry(plugin).or_default().push(b);
        }
        (at, out.into_iter().map(|(k, v)| (k, json!(v))).collect())
    }

    /// Says what badge (`app`, `ticket`) of `id` is now, at once when it has
    /// been quiet for a second, else once the second is over, then with the
    /// latest value. A second's quiet leaves no trace of the badge here.
    fn announce_badge(&self, id: &str, app: &str, ticket: &str) {
        let key = (id.to_owned(), app.to_owned(), ticket.to_owned());
        let now = Instant::now();
        let wait = {
            let mut gates = self.badge_gates.lock().unwrap_or_else(|e| e.into_inner());
            gates.retain(|_, g| g.waiting || g.last.is_some_and(|t| now.duration_since(t) < GAP));
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
                // an uninstall in the meantime took the gate: nothing is left to say
                let mut gates = host.badge_gates.lock().unwrap_or_else(|e| e.into_inner());
                let Some(g) = gates.get_mut(&key) else { return };
                g.waiting = false;
                g.last = Some(Instant::now());
            }
            host.emit_badge(&key.0, &key.1, &key.2);
        });
    }

    /// The event of badge (`app`, `ticket`) of `id` as it is stored now. A read of the
    /// database that fails says nothing (logged once): a badge still stored is not announced as cleared.
    fn emit_badge(&self, id: &str, app: &str, ticket: &str) {
        let now = self.sql(|db| {
            db.query_row("SELECT dot, text, updated_at FROM v1_badges WHERE plugin=? AND app=? AND ticket=?", params![id, app, ticket], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))
                .map(Some)
                .or_else(|e| if matches!(e, rusqlite::Error::QueryReturnedNoRows) { Ok(None) } else { Err(e) })
        });
        let (dot, text, rev) = match now {
            Ok(Some(row)) => row,
            Ok(None) => ("none".to_owned(), String::new(), self.next_rev()),
            Err(e) => return log_once(format!("badge\0{id}\0read"), || format!("  v1 {id}: a badge change was not announced: {}", e.message)),
        };
        let _ = self.badge_events.send(view(Some(id), app, ticket, &dot, &text, rev));
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
            self.badge_gates.lock().unwrap_or_else(|e| e.into_inner()).remove(&(id.to_owned(), app.clone(), ticket.clone()));
            let _ = self.badge_events.send(view(Some(id), &app, &ticket, "none", "", self.next_rev()));
        }
        if keep.is_none() {
            self.badge_gates.lock().unwrap_or_else(|e| e.into_inner()).retain(|k, _| k.0 != id);
        }
    }
}

#[cfg(test)]
#[path = "badges_tests.rs"]
mod tests;
