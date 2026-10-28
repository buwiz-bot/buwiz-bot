//! The one-time migration of v0 BIR tokens to tickets (spec Q11, 16 item 7).
//!
//! v0 keeps a chat's attached tokens and the sidebar's pinned apps in the
//! app server's database (`<broker data dir>/generated/app.db`: the
//! `attachments` table, and `group_items` of kind `app`), and their
//! values are TINs: `@bir:taxprofile:123456789`, `mcp:bir/forms|{"tin":…}`.
//! On the first start with BIR's v1 plugin, each one is handed to the
//! plugin's resolve (`kind: "scope"`, `complete: false`), which issues a
//! ticket, and is rewritten:
//!
//! - an attachment becomes a v1 attachment in Rox's records (canonical
//!   token, plugin id, major, scope, title, chat), and its v0 row's token,
//!   link and key become the canonical token and the v1 link and key (its
//!   `details`, which may name the TIN, are cleared);
//! - a pin becomes a v1 pin (`{plugin, app, ticket?, placement}`) in Rox's
//!   records, and its group item's ref the v1 key.
//!
//! An item whose ticket could not be issued keeps its v0 row as it was and
//! is recorded as "needs update" without its value. The migration runs
//! once (Rox's records remember when) and backs the database up first
//! (`app.db.pre-mini-v1`). It is a function over the two data dirs so it
//! can be rehearsed on a copy; [`Mode::DryRun`] changes nothing and asks
//! the plugin nothing, and reports what it would do. No value (TIN) is
//! ever written to the report or the log.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

use super::{
    cards::Placement,
    links::BIR,
    perms,
    records::{Attachment, Pin, Records, major},
};

/// The v0 server whose tokens are migrated, and its v1 short.
pub const V0_SERVER: &str = "bir";

/// Whether to change anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    DryRun,
    Apply,
}

/// What became (or would become) of one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Rewritten to a ticket.
    Migrated,
    /// Its value is a ticket already: recorded as it is.
    AlreadyTicket,
    /// No value to resolve (a pin of an app with no taxpayer).
    NoValue,
    /// Dry run: would ask the plugin for a ticket.
    WouldResolve,
    /// The plugin could not issue one: "needs update" (the reason).
    NeedsUpdate(String),
}

/// One item of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// `attachment` or `pin`.
    pub kind: &'static str,
    /// The v0 row: the attachment's id, or `<group id>/<app>`.
    pub id: String,
    pub scope: String,
    pub outcome: Outcome,
}

/// What a run did (or would do).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    pub dry_run: bool,
    /// The migration ran before: nothing was looked at.
    pub already: bool,
    pub items: Vec<Item>,
    pub backup: Option<PathBuf>,
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        if self.already {
            return writeln!(f, "The v0 migration already ran; nothing to do.");
        }
        let verb = if self.dry_run { "Would migrate" } else { "Migrated" };
        writeln!(f, "{verb} {} item(s):", self.items.len())?;
        for i in &self.items {
            let what = match &i.outcome {
                Outcome::Migrated => "ticket issued, rewritten".to_owned(),
                Outcome::AlreadyTicket => "already a ticket, recorded".to_owned(),
                Outcome::NoValue => "no taxpayer, recorded".to_owned(),
                Outcome::WouldResolve => "would ask BIR for a ticket and rewrite".to_owned(),
                Outcome::NeedsUpdate(why) => format!("needs update ({why})"),
            };
            writeln!(f, "  {} {} [{}]: {what}", i.kind, i.id, if i.scope.is_empty() { "-" } else { &i.scope })?;
        }
        if let Some(b) = &self.backup {
            writeln!(f, "Backup of the app server's database: {}", b.display())?;
        }
        Ok(())
    }
}

/// The app server's database in its data dir.
pub fn app_db(broker_dir: &Path) -> PathBuf {
    broker_dir.join("generated").join("app.db")
}

/// Asks BIR's v1 plugin for `(ticket, title)` of `value` in `scope`.
pub type Resolve<'a> = dyn FnMut(&str, &str) -> Result<(String, String), String> + 'a;

/// A v0 token's scope and value (`@bir:<scope>:<value>`).
fn v0_token(token: &str) -> Option<(String, String)> {
    let rest = token.trim().strip_prefix(&format!("@{V0_SERVER}:"))?;
    let (scope, value) = rest.split_once(':')?;
    perms::scope_ok(scope).then(|| (scope.to_owned(), value.to_owned()))
}

/// A v0 pin's app and the taxpayer it was pinned for: `mcp:bir/<app>[|<props>]`
/// or `rox://app/bir/<app>[?props=…&token=…]`.
fn v0_pin(r: &str) -> Option<(String, Option<(String, String)>)> {
    let (app, props, token) = if let Some(rest) = r.strip_prefix(&format!("mcp:{V0_SERVER}/")) {
        let (app, props) = rest.split_once('|').unwrap_or((rest, ""));
        (app.to_owned(), props.to_owned(), String::new())
    } else {
        let rest = r.strip_prefix(&format!("rox://app/{V0_SERVER}/"))?;
        let (app, q) = rest.split_once('?').unwrap_or((rest, ""));
        let arg = |k: &str| q.split('&').find_map(|kv| kv.strip_prefix(k)).map(crate::client::urldec).unwrap_or_default();
        (app.trim_end_matches('/').to_owned(), arg("props="), arg("token="))
    };
    if !perms::app_id_ok(&app) {
        return None;
    }
    if let Some(t) = v0_token(&token) {
        return Some((app, Some(t)));
    }
    let props: Value = serde_json::from_str(&props).unwrap_or(Value::Null);
    let tin = props.get("tin").and_then(Value::as_str).filter(|t| !t.is_empty());
    Some((app, tin.map(|t| ("taxprofile".to_owned(), t.to_owned()))))
}

fn table_exists(db: &Connection, name: &str) -> bool {
    db.query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?", [name], |_| Ok(()))
        .optional()
        .ok()
        .flatten()
        .is_some()
}

fn now_ms() -> i64 {
    super::session::now_ms()
}

/// Runs the migration over Rox's data dir `rox_dir` and the app server's
/// `broker_dir`, for BIR's v1 plugin at `version` (its major goes into
/// the attachments). `resolve` is not called in a dry run.
pub fn run(rox_dir: &Path, broker_dir: &Path, version: &str, mode: Mode, resolve: &mut Resolve) -> Result<Report, String> {
    let mut records = Records::load(rox_dir);
    let dry = mode == Mode::DryRun;
    let mut report = Report { dry_run: dry, ..Default::default() };
    if records.migrated_at > 0 {
        report.already = true;
        return Ok(report);
    }
    let path = app_db(broker_dir);
    let db = if path.exists() {
        let db = Connection::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let _ = db.busy_timeout(std::time::Duration::from_secs(5));
        Some(db)
    } else {
        None
    };
    if let Some(db) = db.as_ref().filter(|_| !dry) {
        let backup = path.with_file_name("app.db.pre-mini-v1");
        if !backup.exists() {
            db.execute("VACUUM INTO ?", [backup.to_string_lossy()]).map_err(|e| format!("backup: {e}"))?;
        }
        report.backup = Some(backup);
    }
    let short = V0_SERVER;
    let major = major(version);
    // the tickets issued, by (scope, value): a TIN attached in two chats
    // is asked about once
    let mut cache: Vec<((String, String), Result<(String, String), String>)> = Vec::new();
    let mut issue = |scope: &str, value: &str| -> (Outcome, Option<(String, String)>) {
        if perms::ticket_ok(value) {
            return (Outcome::AlreadyTicket, Some((value.to_owned(), String::new())));
        }
        if dry {
            return (Outcome::WouldResolve, None);
        }
        let r = match cache.iter().find(|(k, _)| k.0 == scope && k.1 == value) {
            Some((_, r)) => r.clone(),
            None => {
                let r = match resolve(scope, value) {
                    Ok((ticket, title)) if perms::ticket_ok(&ticket) => Ok((ticket, title)),
                    Ok(_) => Err("the plugin sent no ticket".to_owned()),
                    Err(why) => Err(why.chars().take(200).collect()),
                };
                cache.push(((scope.to_owned(), value.to_owned()), r.clone()));
                r
            }
        };
        match r {
            Ok(t) => (Outcome::Migrated, Some(t)),
            Err(why) => (Outcome::NeedsUpdate(why), None),
        }
    };
    let Some(db) = db else {
        if !dry {
            records.migrated_at = now_ms();
            records.save(rox_dir).map_err(|e| e.to_string())?;
        }
        return Ok(report);
    };
    // attachments
    if table_exists(&db, "attachments") {
        let rows: Vec<(String, String, String, String, String)> = {
            let mut st = db
                .prepare("SELECT id, token, COALESCE(title,''), thread, COALESCE(open,'') FROM attachments WHERE server=? ORDER BY created_at")
                .map_err(|e| e.to_string())?;
            st.query_map([V0_SERVER], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
                .map_err(|e| e.to_string())?
                .filter_map(Result::ok)
                .collect()
        };
        for (id, token, title, thread, open) in rows {
            let Some((scope, value)) = v0_token(&token) else { continue };
            let (outcome, got) = issue(&scope, &value);
            report.items.push(Item { kind: "attachment", id: id.clone(), scope: scope.clone(), outcome: outcome.clone() });
            if dry {
                continue;
            }
            match got {
                Some((ticket, resolved_title)) => {
                    let canonical = format!("@{short}:{scope}:{ticket}");
                    let title = if resolved_title.is_empty() { title } else { resolved_title };
                    // the app its v0 link opened, now with the ticket
                    let (link, key) = match v0_pin(&open) {
                        Some((app, _)) => {
                            (format!("rox://app/{BIR}/{app}?ticket={ticket}"), format!("v1:{BIR}/{app}?ticket={ticket}"))
                        }
                        None => (String::new(), String::new()),
                    };
                    db.execute(
                        "UPDATE attachments SET token=?, open=?, key=?, details=NULL, title=? WHERE id=?",
                        rusqlite::params![canonical, link, key, title, id],
                    )
                    .map_err(|e| e.to_string())?;
                    if !records.attachments.iter().any(|a| a.thread == thread && a.token == canonical) {
                        records.attachments.push(Attachment {
                            token: canonical,
                            plugin: BIR.to_owned(),
                            major,
                            scope,
                            title,
                            thread,
                            needs_update: false,
                        });
                    }
                }
                None => records.attachments.push(Attachment {
                    token: String::new(),
                    plugin: BIR.to_owned(),
                    major,
                    scope,
                    title,
                    thread,
                    needs_update: true,
                }),
            }
        }
    }
    // pins
    if table_exists(&db, "group_items") {
        let rows: Vec<(String, String, String)> = {
            let mut st = db
                .prepare("SELECT group_id, ref, COALESCE(title,'') FROM group_items WHERE kind='app' ORDER BY ord")
                .map_err(|e| e.to_string())?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(|e| e.to_string())?
                .filter_map(Result::ok)
                .collect()
        };
        for (group, r, title) in rows {
            let Some((app, subject)) = v0_pin(&r) else { continue };
            let id = format!("{group}/{app}");
            let (outcome, got) = match &subject {
                None => (Outcome::NoValue, None),
                Some((scope, value)) => issue(scope, value),
            };
            let scope = subject.as_ref().map(|s| s.0.clone()).unwrap_or_default();
            report.items.push(Item { kind: "pin", id, scope, outcome: outcome.clone() });
            if dry {
                continue;
            }
            if let Outcome::NeedsUpdate(_) = outcome {
                // recorded without its value; the v0 row stays as it was
                let placement = Placement::Full.as_str().into();
                records.pins.push(Pin { plugin: BIR.into(), app, ticket: None, placement, title, needs_update: true });
                continue;
            }
            let ticket = got.map(|g| g.0);
            let key = match &ticket {
                Some(t) => format!("v1:{BIR}/{app}?ticket={t}"),
                None => format!("v1:{BIR}/{app}"),
            };
            db.execute(
                "UPDATE OR REPLACE group_items SET ref=? WHERE group_id=? AND kind='app' AND ref=?",
                rusqlite::params![key, group, r],
            )
            .map_err(|e| e.to_string())?;
            let placement = Placement::Full.as_str().into();
            records.pin(Pin { plugin: BIR.into(), app, ticket, placement, title, needs_update: false });
        }
    }
    if !dry {
        records.migrated_at = now_ms();
        records.save(rox_dir).map_err(|e| e.to_string())?;
        log::info!("mini v1: migrated {} v0 item(s) of {V0_SERVER}", report.items.len());
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T1: &str = "tkt_0123456789ABCDEFGHJKMNPQRS";
    const T2: &str = "tkt_ZZZZZZZZZZZZZZZZZZZZZZZZZZ";
    const TIN: &str = "987654321";

    /// A fixture of both data dirs: Rox's (empty) and the app server's
    /// with v0 BIR attachments and pins, as v0 writes them.
    fn fixture(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("rox-migrate-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (rox, broker) = (base.join("rox"), base.join("grok"));
        std::fs::create_dir_all(broker.join("generated")).unwrap();
        let db = Connection::open(app_db(&broker)).unwrap();
        db.execute_batch(
            "CREATE TABLE attachments(id TEXT PRIMARY KEY, server TEXT NOT NULL, token TEXT NOT NULL, scope TEXT,
               title TEXT, open TEXT, key TEXT, details TEXT, thread TEXT NOT NULL, bot TEXT, turn TEXT, created_at INTEGER);
             CREATE TABLE group_items(group_id TEXT NOT NULL, kind TEXT NOT NULL, ref TEXT NOT NULL, thread TEXT,
               ord INTEGER NOT NULL DEFAULT 0, title TEXT, PRIMARY KEY(group_id, kind, ref));",
        )
        .unwrap();
        let open = format!("rox://app/bir/tax-profile?props=%7B%22tin%22%3A%22{TIN}%22%7D&token=%40bir%3Ataxprofile%3A{TIN}");
        let rows = [
            ("a1", "bir", format!("@bir:taxprofile:{TIN}"), "Dela Cruz Jane", open.as_str(), "th1", 1),
            ("a2", "bir", format!("@bir:taxprofile:{TIN}"), "Dela Cruz Jane", open.as_str(), "th2", 2),
            ("a3", "bir", "@bir:taxprofile:000".to_owned(), "Unknown", "", "th1", 3),
            ("a4", "bir", format!("@bir:branchcode:{T2}"), "Branch", "", "th1", 4),
            ("a5", "gws", "@gws:mail:abc".to_owned(), "Mail", "", "th1", 5),
        ];
        for (id, server, token, title, open, thread, at) in rows {
            db.execute(
                "INSERT INTO attachments(id,server,token,scope,title,open,key,details,thread,created_at) VALUES(?,?,?,?,?,?,?,?,?,?)",
                rusqlite::params![id, server, token, "taxprofile", title, open, format!("mcp:bir/tax-profile|{{\"tin\":\"{TIN}\"}}"), format!("{{\"subtitle\":\"TIN {TIN}\"}}"), thread, at],
            )
            .unwrap();
        }
        let pins = [
            ("g1", format!("mcp:bir/forms|{{\"tin\":\"{TIN}\",\"form\":\"1601C\"}}"), "Forms for Jane"),
            ("g1", "mcp:bir/calendar".to_owned(), "Calendar"),
            ("g2", "mcp:bir/forms|{\"tin\":\"000\"}".to_owned(), "Forms for nobody"),
            ("g2", "mcp:other/app".to_owned(), "Other"),
        ];
        for (i, (g, r, title)) in pins.into_iter().enumerate() {
            db.execute("INSERT INTO group_items(group_id,kind,ref,ord,title) VALUES(?,'app',?,?,?)", rusqlite::params![g, r, i as i64, title])
                .unwrap();
        }
        (base, rox, broker)
    }

    fn dump(broker: &Path) -> String {
        let db = Connection::open(app_db(broker)).unwrap();
        let mut out = String::new();
        for q in ["SELECT id||' '||token||' '||COALESCE(open,'')||' '||COALESCE(key,'')||' '||COALESCE(details,'') FROM attachments ORDER BY id",
                  "SELECT group_id||' '||ref FROM group_items ORDER BY group_id, ord"] {
            let mut st = db.prepare(q).unwrap();
            for r in st.query_map([], |r| r.get::<_, String>(0)).unwrap() {
                out.push_str(&r.unwrap());
                out.push('\n');
            }
        }
        out
    }

    fn bir(scope: &str, value: &str) -> Result<(String, String), String> {
        assert_eq!(scope, "taxprofile");
        match value {
            TIN => Ok((T1.to_owned(), "Dela Cruz Jane".to_owned())),
            _ => Err(format!("No taxpayer {}", "x".repeat(value.len()))),
        }
    }

    #[test]
    fn q11_a_dry_run_reports_and_changes_nothing() {
        let (base, rox, broker) = fixture("dry");
        let before = dump(&broker);
        let mut asked = 0;
        let mut never = |_: &str, _: &str| {
            asked += 1;
            Err("asked".to_owned())
        };
        let r = run(&rox, &broker, "1.0.0", Mode::DryRun, &mut never).unwrap();
        assert_eq!(asked, 0, "a dry run asks the plugin nothing");
        assert_eq!(dump(&broker), before);
        assert!(!records_file_exists(&rox));
        assert!(r.backup.is_none());
        let kinds: Vec<(&str, &str, &Outcome)> = r.items.iter().map(|i| (i.kind, i.id.as_str(), &i.outcome)).collect();
        assert_eq!(kinds, [
            ("attachment", "a1", &Outcome::WouldResolve),
            ("attachment", "a2", &Outcome::WouldResolve),
            ("attachment", "a3", &Outcome::WouldResolve),
            ("attachment", "a4", &Outcome::AlreadyTicket),
            ("pin", "g1/forms", &Outcome::WouldResolve),
            ("pin", "g1/calendar", &Outcome::NoValue),
            ("pin", "g2/forms", &Outcome::WouldResolve),
        ]);
        let text = r.to_string();
        assert!(text.starts_with("Would migrate 7 item(s):"), "{text}");
        assert!(!text.contains(TIN), "the report never names a TIN");
        let _ = std::fs::remove_dir_all(&base);
    }

    fn records_file_exists(rox: &Path) -> bool {
        super::super::records::file(rox).exists()
    }

    #[test]
    fn q11_the_migration_rewrites_tins_to_tickets_and_failures_need_update() {
        let (base, rox, broker) = fixture("apply");
        let mut asked = Vec::new();
        let mut resolve = |scope: &str, value: &str| {
            asked.push(value.len());
            bir(scope, value)
        };
        let r = run(&rox, &broker, "1.2.0", Mode::Apply, &mut resolve).unwrap();
        // the same TIN is asked about once, the bad one once
        assert_eq!(asked.len(), 2);
        let after = dump(&broker);
        assert!(!after.lines().filter(|l| !l.starts_with("a3") && !l.starts_with("a5") && !l.starts_with("g2 mcp:bir")).any(|l| l.contains(TIN)), "{after}");
        assert!(after.contains(&format!("a1 @bir:taxprofile:{T1} rox://app/{BIR}/tax-profile?ticket={T1} v1:{BIR}/tax-profile?ticket={T1} \n")), "{after}");
        assert!(after.contains(&format!("g1 v1:{BIR}/forms?ticket={T1}\n")), "{after}");
        assert!(after.contains(&format!("g1 v1:{BIR}/calendar\n")), "{after}");
        // a failure keeps its v0 row as it was
        assert!(after.contains("a3 @bir:taxprofile:000"), "{after}");
        assert!(after.contains("g2 mcp:bir/forms|{\"tin\":\"000\"}"), "{after}");
        assert!(after.contains("a5 @gws:mail:abc"), "other servers are untouched");
        // Rox's records: canonical tokens, the plugin major, needs update
        let rec = Records::load(&rox);
        let tokens: Vec<(&str, &str, bool)> = rec.attachments.iter().map(|a| (a.thread.as_str(), a.token.as_str(), a.needs_update)).collect();
        let t1 = format!("@bir:taxprofile:{T1}");
        let t2 = format!("@bir:branchcode:{T2}");
        assert_eq!(tokens, [("th1", t1.as_str(), false), ("th2", t1.as_str(), false), ("th1", "", true), ("th1", t2.as_str(), false)]);
        assert!(rec.attachments.iter().all(|a| a.plugin == BIR && a.major == 1));
        assert_eq!(rec.pins.len(), 3);
        assert_eq!((rec.pins[0].app.as_str(), rec.pins[0].ticket.as_deref()), ("forms", Some(T1)));
        assert!(rec.pins[2].needs_update && rec.pins[2].ticket.is_none());
        assert!(!serde_json::to_string(&rec.to_json()).unwrap().contains(TIN), "Rox's records hold no TIN");
        assert!(rec.migrated_at > 0);
        assert!(r.backup.as_ref().is_some_and(|b| b.exists()));
        assert_eq!(r.items.iter().filter(|i| matches!(i.outcome, Outcome::NeedsUpdate(_))).count(), 2);
        assert!(!r.to_string().contains(TIN));
        // once only
        let again = run(&rox, &broker, "1.2.0", Mode::Apply, &mut |_: &str, _: &str| Err("asked".to_owned())).unwrap();
        assert!(again.already && again.items.is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn q11_no_app_server_database_marks_the_migration_done() {
        let base = std::env::temp_dir().join(format!("rox-migrate-{}-empty", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let r = run(&base.join("rox"), &base.join("grok"), "1.0.0", Mode::Apply, &mut |_: &str, _: &str| Err("x".into())).unwrap();
        assert!(r.items.is_empty());
        assert!(Records::load(&base.join("rox")).migrated_at > 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn v0_pins_and_tokens_name_their_app_and_taxpayer() {
        assert_eq!(v0_token(&format!("@bir:taxprofile:{TIN}")), Some(("taxprofile".into(), TIN.into())));
        assert_eq!(v0_token("@gws:mail:x"), None);
        assert_eq!(v0_pin("mcp:bir/forms|{\"tin\":\"1\"}"), Some(("forms".into(), Some(("taxprofile".into(), "1".into())))));
        assert_eq!(v0_pin("mcp:bir/calendar"), Some(("calendar".into(), None)));
        assert_eq!(
            v0_pin("rox://app/bir/forms?props=%7B%7D&token=%40bir%3Abranchcode%3A9"),
            Some(("forms".into(), Some(("branchcode".into(), "9".into()))))
        );
        assert_eq!(v0_pin("mcp:other/app"), None);
        assert_eq!(v0_pin("v1:dev.goldcoders.bir/forms"), None);
    }
}
