//! The sidebar's user-made groups (groups.mjs): a group holds bots, chats,
//! artifacts and server mini apps in the user's order; rows written one
//! change at a time. An artifact member's Library record is the Library's
//! (5.7b, track C): the [`ArtifactRecords`] seam in `ctx.ext`; without it
//! no artifact is found (skipped in the list, and kept).

use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

use super::attachments::renamed_keys;
use crate::hub::ctx::HubCtx;
use crate::hub::store::json::{js_number, js_slice, template};
use crate::hub::store::write::to_json;
use crate::hub::store::{now_ms, State};
use crate::hub::types::BoxFuture;
use crate::js;

/// artifacts.mjs `artifactRecords(items, origin)`: for each `(id, thread)`
/// its Library record (with `title` and `origin.thread`), or `None`.
pub type RecordsFn = dyn Fn(Arc<HubCtx>, Vec<(String, Option<String>)>, String) -> BoxFuture<'static, Vec<Option<Value>>> + Send + Sync;

/// The seam in `ctx.ext` (track C fills it).
#[derive(Clone)]
pub struct ArtifactRecords(pub Arc<RecordsFn>);

/// The color dots a group picks from.
pub const GROUP_COLORS: [&str; 8] = ["#1884fc", "#32bb78", "#f2994a", "#eb5757", "#9b51e0", "#2d9cdb", "#f2c94c", "#8e8e93"];
const KINDS: [&str; 4] = ["bot", "chat", "artifact", "app"];

fn base36(mut n: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(D[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// `` `${prefix}${Date.now().toString(36)}${Math.floor(Math.random() * 46656).toString(36)}` ``
pub fn time_id(prefix: &str) -> String {
    let r = u16::from_le_bytes(buwiz_protocol::keys::random::<2>()) as u64 * 46656 / 65536;
    format!("{prefix}{}{}", base36(now_ms() as u64), base36(r))
}

/// `Date.now().toString(36)`.
pub fn now36() -> String {
    base36(now_ms() as u64)
}

/// `Math.random().toString(36).slice(2, 6)`.
pub fn random36(n: usize) -> String {
    let b = buwiz_protocol::keys::random::<8>();
    b.iter().take(n).map(|x| base36((*x as u64) % 36)).collect()
}

/// `String(name ?? "").trim().slice(0, 60)`.
fn clean_name(name: Option<&Value>) -> String {
    let s = match name {
        None | Some(Value::Null) => String::new(),
        Some(v) => js::string_of(v),
    };
    js_slice(s.trim_matches(js::is_space), 60)
}

/// A `#rrggbb` color, lowercased, or "".
fn clean_color(c: Option<&Value>) -> String {
    let s = c.filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
    let ok = s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit());
    if ok {
        s.to_lowercase()
    } else {
        String::new()
    }
}

/// `"R18@v1"` → `"R18"`.
fn art_id(r: &str) -> String {
    if let Some((n, v)) = r.split_once("@v") {
        let rn = n.strip_prefix('R').is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
        if rn && !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) {
            return n.into();
        }
    }
    r.into()
}

fn is_r(r: &str) -> bool {
    r.strip_prefix('R').is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

fn line_end(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// `/^mcp:[a-z0-9-]+\/[A-Za-z0-9._-]+(\|\{.*\})?$/s`
fn app_ref(r: &str) -> bool {
    let Some(rest) = r.strip_prefix("mcp:") else { return false };
    let Some((server, rest)) = rest.split_once('/') else { return false };
    if server.is_empty() || !server.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return false;
    }
    let app_len = rest.bytes().take_while(|b| b.is_ascii_alphanumeric() || b"._-".contains(b)).count();
    if app_len == 0 {
        return false;
    }
    let tail = &rest[app_len..];
    tail.is_empty() || (tail.starts_with("|{") && tail.ends_with('}') && tail.len() >= 3)
}

/// `ref.replace(/^mcp:|\|.*$/g, "")`
fn app_title(r: &str) -> String {
    let r = r.strip_prefix("mcp:").unwrap_or(r);
    // `\|.*$` matches from a "|" on the last line to the end
    let last_line = r.rfind(line_end).map(|i| i + r[i..].chars().next().map(char::len_utf8).unwrap_or(1)).unwrap_or(0);
    match r[last_line..].find('|') {
        Some(k) => r[..last_line + k].to_owned(),
        None => r.to_owned(),
    }
}

/// `/^(file|gdrive):./`
fn file_ref(r: &str) -> bool {
    ["file:", "gdrive:"].iter().any(|p| r.strip_prefix(p).and_then(|x| x.chars().next()).is_some_and(|c| !line_end(c)))
}

type Item = (String, String, Option<String>, Option<String>);

fn group_ids(c: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut st = c.prepare("SELECT id FROM groups ORDER BY ord, created_at")?;
    let ids = st.query_map([], |r| Ok(js::string_of(&to_json(r.get(0)?))))?.collect();
    ids
}

fn item_rows(c: &Connection, gid: &str) -> rusqlite::Result<Vec<Item>> {
    let mut st = c.prepare("SELECT kind, ref, thread, title FROM group_items WHERE group_id=? ORDER BY ord")?;
    let t = |v: Value| if v.is_null() { None } else { Some(js::string_of(&v)) };
    let items = st.query_map([gid], |r| Ok((js::string_of(&to_json(r.get(0)?)), js::string_of(&to_json(r.get(1)?)), t(to_json(r.get(2)?)), t(to_json(r.get(3)?)))))?.collect();
    items
}

fn group_exists(c: &Connection, id: &str) -> rusqlite::Result<Option<String>> {
    c.query_row("SELECT id FROM groups WHERE id=?", [id], |r| Ok(js::string_of(&to_json(r.get(0)?)))).optional()
}

fn tx<T>(c: &Connection, f: impl FnOnce(&Connection) -> Result<T, String>) -> Result<T, String> {
    c.execute_batch("BEGIN IMMEDIATE").map_err(|e| e.to_string())?;
    match f(c) {
        Ok(v) => {
            c.execute_batch("COMMIT").map_err(|e| e.to_string())?;
            Ok(v)
        }
        Err(e) => {
            let _ = c.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

fn e(x: rusqlite::Error) -> String {
    x.to_string()
}

fn renumber_items(c: &Connection, gid: &str, items: &[(String, String)]) -> Result<(), String> {
    for (k, (kind, r)) in items.iter().enumerate() {
        c.execute("UPDATE group_items SET ord=? WHERE group_id=? AND kind=? AND ref=?", rusqlite::params![k as i64, gid, kind, r]).map_err(e)?;
    }
    Ok(())
}

fn insert_group(c: &Connection, name: &str, color: Option<&Value>) -> Result<String, String> {
    let n: i64 = c.query_row("SELECT COUNT(*) FROM groups", [], |r| r.get(0)).map_err(e)?;
    let id = time_id("g");
    let color = match clean_color(color) {
        x if !x.is_empty() => x,
        _ => GROUP_COLORS[(n as usize) % GROUP_COLORS.len()].to_owned(),
    };
    c.execute("INSERT INTO groups(id,name,color,ord,collapsed,created_at) VALUES(?,?,?,?,0,?)", rusqlite::params![id, crate::hub::store::json::sql_text(name), color, n, now_ms()]).map_err(e)?;
    Ok(id)
}

/// `putItem`: adds (or, already there, moves) one member to `index`.
fn put_item(c: &Connection, gid: &str, kind: &str, r: &str, thread: Option<&str>, index: Option<f64>, title: Option<&str>) -> Result<(), String> {
    let cur = item_rows(c, gid).map_err(e)?;
    let mut items: Vec<(String, String)> = cur.iter().filter(|it| !(it.0 == kind && it.1 == r)).map(|it| (it.0.clone(), it.1.clone())).collect();
    let present = items.len() != cur.len();
    let title = title.filter(|t| !t.is_empty());
    if !present {
        c.execute("INSERT INTO group_items(group_id,kind,ref,thread,ord,title) VALUES(?,?,?,?,?,?)", rusqlite::params![gid, kind, r, thread, items.len() as i64, title]).map_err(e)?;
    } else if let Some(t) = thread {
        c.execute("UPDATE group_items SET thread=? WHERE group_id=? AND kind=? AND ref=?", rusqlite::params![t, gid, kind, r]).map_err(e)?;
    }
    if let (Some(t), true) = (title, present) {
        c.execute("UPDATE group_items SET title=? WHERE group_id=? AND kind=? AND ref=?", rusqlite::params![t, gid, kind, r]).map_err(e)?;
    }
    let at = match index {
        Some(i) if i.is_finite() && i.fract() == 0.0 => (i.max(0.0) as usize).min(items.len()),
        _ => items.len(),
    };
    items.insert(at, (kind.into(), r.into()));
    renumber_items(c, gid, &items)
}

/// `migrateGroups()`: once (meta groups_migrated), each bot section becomes
/// a group of its bots and the pinned Mini Apps one more.
pub fn migrate(ctx: &HubCtx) -> Result<bool, String> {
    let done = ctx.db.with(|c| c.query_row("SELECT v FROM meta WHERE k='groups_migrated'", [], |r| r.get::<_, rusqlite::types::Value>(0)).optional())?;
    if done.is_some_and(|v| js::truthy(&to_json(v))) {
        return Ok(false);
    }
    let (sections, bots) = ctx.store.read(|s| (s.sections.as_array().cloned().unwrap_or_default(), s.bots.iter().map(|b| (b.id.clone(), b.section.clone())).collect::<Vec<_>>()));
    ctx.db.with(|c| {
        Ok(tx(c, |c| {
            let mut names: Vec<String> = sections.iter().map(js::string_of).collect();
            for (_, sec) in &bots {
                if !sec.is_empty() && !names.contains(sec) {
                    names.push(sec.clone());
                }
            }
            for name in &names {
                let members: Vec<&String> = bots.iter().filter(|(_, s)| s == name).map(|(id, _)| id).collect();
                let clean = clean_name(Some(&json!(name)));
                if members.is_empty() || clean.is_empty() {
                    continue;
                }
                let gid = insert_group(c, &clean, None)?;
                for b in members {
                    put_item(c, &gid, "bot", b, None, None, None)?;
                }
            }
            let row: Option<Option<String>> = c.query_row("SELECT data FROM kv WHERE name='rox-pinned-apps'", [], |r| r.get(0)).optional().unwrap_or(None);
            let pinned: Vec<Value> = row
                .flatten()
                .and_then(|d| serde_json::from_str::<Value>(if d.is_empty() { "{}" } else { &d }).ok())
                .and_then(|v| v.get("apps").cloned())
                .and_then(|a| a.as_array().cloned())
                .unwrap_or_default()
                .into_iter()
                .filter(|p| p.get("app").is_some_and(js::truthy))
                .collect();
            if !pinned.is_empty() {
                let gid = insert_group(c, "Mini Apps", None)?;
                for p in pinned {
                    let thread = p.get("thread").filter(|t| js::truthy(t)).map(js::string_of);
                    put_item(c, &gid, "artifact", &art_id(&js::string_of(&p["app"])), thread.as_deref(), None, None)?;
                }
            }
            c.execute("INSERT OR REPLACE INTO meta(k,v) VALUES(?,?)", rusqlite::params!["groups_migrated", now_ms().to_string()]).map_err(e)?;
            Ok(())
        }))
    })??;
    Ok(true)
}

/// `GET /groups`: every group with its members resolved; a member whose
/// bot or chat is gone is dropped for good.
pub async fn list(ctx: Arc<HubCtx>, origin: String) -> Result<Value, String> {
    let (groups, all) = ctx.db.with(|c| {
        let mut st = c.prepare("SELECT id, name, color, collapsed FROM groups ORDER BY ord, created_at")?;
        let gs: Vec<(String, Value, Value, Value)> =
            st.query_map([], |r| Ok((js::string_of(&to_json(r.get(0)?)), to_json(r.get(1)?), to_json(r.get(2)?), to_json(r.get(3)?))))?.collect::<rusqlite::Result<_>>()?;
        let all: Vec<Vec<Item>> = gs.iter().map(|g| item_rows(c, &g.0)).collect::<rusqlite::Result<_>>()?;
        Ok((gs, all))
    })?;
    let wanted: Vec<(String, Option<String>)> = all.iter().flatten().filter(|it| it.0 == "artifact").map(|it| (it.1.clone(), it.2.clone())).collect();
    let arts = match ctx.ext.get::<ArtifactRecords>() {
        Some(f) if !wanted.is_empty() => (f.0)(ctx.clone(), wanted.clone(), origin).await,
        _ => vec![None; wanted.len()],
    };
    let renamed = ctx.db.with(|c| Ok(renamed_keys(c)))?;
    let mut a = 0;
    let mut gone: Vec<(String, String, String)> = Vec::new();
    let out: Vec<Value> = ctx.store.read(|s: &State| {
        groups
            .iter()
            .enumerate()
            .map(|(k, (id, name, color, collapsed))| {
                let items: Vec<Value> = all[k]
                    .iter()
                    .filter_map(|(kind, r, thread, title)| match kind.as_str() {
                        "bot" => match s.bot(r) {
                            Some(b) => Some(json!({"kind": "bot", "ref": r, "title": b.name, "hidden": b.hidden})),
                            None => {
                                gone.push((id.clone(), kind.clone(), r.clone()));
                                None
                            }
                        },
                        "app" => {
                            let t = renamed.get(r).filter(|t| js::truthy(t)).cloned().or_else(|| title.clone().filter(|t| !t.is_empty()).map(Value::String)).unwrap_or_else(|| json!(app_title(r)));
                            Some(json!({"kind": "app", "ref": r, "title": t}))
                        }
                        "chat" => match s.list.iter().find(|t| &t.id == r) {
                            Some(t) => Some(json!({"kind": "chat", "ref": r, "title": t.title, "hidden": t.hidden || t.archived})),
                            None => {
                                gone.push((id.clone(), kind.clone(), r.clone()));
                                None
                            }
                        },
                        _ => {
                            let rec = arts.get(a).cloned().flatten();
                            a += 1;
                            let rec = rec?;
                            let thread = thread.clone().filter(|t| !t.is_empty()).map(Value::String).unwrap_or_else(|| rec.get("origin").and_then(|o| o.get("thread")).cloned().unwrap_or(Value::Null));
                            Some(json!({"kind": "artifact", "ref": r, "thread": thread, "title": rec.get("title").cloned().unwrap_or(Value::Null), "artifact": rec}))
                        }
                    })
                    .collect();
                json!({"id": id, "name": name, "color": color, "order": k, "collapsed": js::truthy(collapsed), "items": items})
            })
            .collect()
    });
    if !gone.is_empty() {
        ctx.db.with(|c| {
            Ok(tx(c, |c| {
                for (gid, kind, r) in &gone {
                    c.execute("DELETE FROM group_items WHERE group_id=? AND kind=? AND ref=?", [gid, kind, r]).map_err(e)?;
                }
                Ok(())
            }))
        })??;
    }
    Ok(json!({"groups": out, "colors": GROUP_COLORS}))
}

/// `checkItem`: a member to add, checked against the store.
fn check_item(s: &State, item: &Value) -> Result<(String, String, Option<String>, Option<String>), String> {
    let kind = item.get("kind").cloned().unwrap_or(Value::Null);
    let k = kind.as_str().filter(|k| KINDS.contains(k)).ok_or_else(|| format!("bad kind {}", template(item.get("kind"))))?;
    let r = item.get("ref").filter(|r| js::truthy(r)).map(js::string_of).unwrap_or_default();
    if k == "app" {
        if !app_ref(&r) {
            return Err(format!("bad app {r}"));
        }
        let title = item.get("title").filter(|t| js::truthy(t)).map(js::string_of).unwrap_or_default();
        return Ok((k.into(), r, None, Some(js_slice(title.trim_matches(js::is_space), 80))));
    }
    let r = if k == "artifact" { art_id(&r) } else { r };
    if k == "bot" && s.bot(&r).is_none() {
        return Err(format!("unknown bot {r}"));
    }
    if k == "chat" && !s.list.iter().any(|t| t.id == r) {
        return Err(format!("unknown chat {r}"));
    }
    if k == "artifact" && !is_r(&r) && !file_ref(&r) {
        return Err(format!("bad artifact id {r}"));
    }
    let thread = item.get("thread").and_then(Value::as_str).filter(|t| k == "artifact" && s.list.iter().any(|x| x.id == *t)).map(str::to_owned);
    Ok((k.into(), r, thread, None))
}

/// `POST /group/new {name, color?, item?}`.
pub fn new_group(ctx: &HubCtx, b: &Value) -> Result<Value, String> {
    let name = clean_name(b.get("name"));
    if name.is_empty() {
        return Err("a group needs a name".into());
    }
    let it = match b.get("item").filter(|i| js::truthy(i)) {
        Some(item) => Some(ctx.store.read(|s| check_item(s, item))?),
        None => None,
    };
    let id = ctx.db.with(|c| {
        Ok(tx(c, |c| {
            let id = insert_group(c, &name, b.get("color"))?;
            if let Some((k, r, t, title)) = &it {
                put_item(c, &id, k, r, t.as_deref(), None, title.as_deref())?;
            }
            Ok(id)
        }))
    })??;
    Ok(json!({"id": id}))
}

/// `POST /group/update {id, name?, color?, collapsed?, order?}`.
pub fn update_group(ctx: &HubCtx, b: &Value) -> Result<Value, String> {
    let id = b.get("id").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
    let Some(gid) = ctx.db.with(|c| group_exists(c, &id))? else { return Err(format!("unknown group {}", template(b.get("id")))) };
    ctx.db.with(|c| {
        Ok(tx(c, |c| {
            if let Some(name) = b.get("name") {
                let n = clean_name(Some(name));
                if n.is_empty() {
                    return Err("a group needs a name".into());
                }
                c.execute("UPDATE groups SET name=? WHERE id=?", rusqlite::params![crate::hub::store::json::sql_text(&n), gid]).map_err(e)?;
            }
            if let Some(color) = b.get("color") {
                let col = clean_color(Some(color));
                if col.is_empty() {
                    return Err(format!("bad color {}", js::string_of(color)));
                }
                c.execute("UPDATE groups SET color=? WHERE id=?", rusqlite::params![col, gid]).map_err(e)?;
            }
            if let Some(collapsed) = b.get("collapsed") {
                c.execute("UPDATE groups SET collapsed=? WHERE id=?", rusqlite::params![js::truthy(collapsed) as i64, gid]).map_err(e)?;
            }
            if let Some(order) = b.get("order") {
                let mut ids: Vec<String> = group_ids(c).map_err(e)?.into_iter().filter(|x| *x != gid).collect();
                let n = js_number(Some(order));
                let n = if n.is_nan() || n == 0.0 { 0.0 } else { n };
                let at = (n.min(ids.len() as f64).max(0.0)).trunc() as usize;
                ids.insert(at.min(ids.len()), gid.clone());
                for (k, x) in ids.iter().enumerate() {
                    c.execute("UPDATE groups SET ord=? WHERE id=?", rusqlite::params![k as i64, x]).map_err(e)?;
                }
            }
            Ok(())
        }))
    })??;
    Ok(json!({}))
}

/// `POST /group/delete {id}`: the group goes, its members stay.
pub fn delete_group(ctx: &HubCtx, b: &Value) -> Result<Value, String> {
    let id = b.get("id").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
    let Some(gid) = ctx.db.with(|c| group_exists(c, &id))? else { return Err(format!("unknown group {}", template(b.get("id")))) };
    ctx.db.with(|c| {
        Ok(tx(c, |c| {
            c.execute("DELETE FROM group_items WHERE group_id=?", [&gid]).map_err(e)?;
            c.execute("DELETE FROM groups WHERE id=?", [&gid]).map_err(e)?;
            for (k, x) in group_ids(c).map_err(e)?.iter().enumerate() {
                c.execute("UPDATE groups SET ord=? WHERE id=?", rusqlite::params![k as i64, x]).map_err(e)?;
            }
            Ok(())
        }))
    })??;
    Ok(json!({}))
}

/// `POST /group/item {id, kind, ref, thread?, title?, op, index?}`.
pub fn group_item(ctx: &HubCtx, b: &Value) -> Result<Value, String> {
    let id = b.get("id").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
    let Some(gid) = ctx.db.with(|c| group_exists(c, &id))? else { return Err(format!("unknown group {}", template(b.get("id")))) };
    let op = b.get("op");
    if op.and_then(Value::as_str) == Some("remove") {
        let kind = b.get("kind").and_then(Value::as_str).filter(|k| KINDS.contains(k)).ok_or_else(|| format!("bad kind {}", template(b.get("kind"))))?;
        let r = b.get("ref").filter(|r| js::truthy(r)).map(js::string_of).unwrap_or_default();
        let r = if kind == "artifact" { art_id(&r) } else { r };
        ctx.db.with(|c| {
            Ok(tx(c, |c| {
                c.execute("DELETE FROM group_items WHERE group_id=? AND kind=? AND ref=?", [&gid, kind, &r]).map_err(e)?;
                let items: Vec<(String, String)> = item_rows(c, &gid).map_err(e)?.into_iter().map(|it| (it.0, it.1)).collect();
                renumber_items(c, &gid, &items)
            }))
        })??;
        return Ok(json!({}));
    }
    if op.and_then(Value::as_str) != Some("add") {
        return Err(format!("unknown op {}", template(op)));
    }
    let (k, r, t, title) = ctx.store.read(|s| check_item(s, b))?;
    let index = b.get("index").filter(|i| !i.is_null()).map(|i| js_number(Some(i)));
    ctx.db.with(|c| Ok(tx(c, |c| put_item(c, &gid, &k, &r, t.as_deref(), index, title.as_deref()))))??;
    Ok(json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_refs_read_as_groups_mjs_reads_them() {
        assert_eq!(art_id("R18@v1"), "R18");
        assert_eq!(art_id("R18@vx"), "R18@vx");
        assert!(app_ref("mcp:apps/hello"));
        assert!(app_ref("mcp:apps/hello|{\"a\":1}"));
        assert!(!app_ref("apps/hello"));
        assert!(!app_ref("mcp:apps/hello|x"));
        assert_eq!(app_title("mcp:apps/hello|{\"a\":1}"), "apps/hello");
        assert_eq!(app_title("mcp:apps/plain"), "apps/plain");
        assert!(file_ref("file:m1/a.txt") && file_ref("gdrive:x") && !file_ref("file:") && !file_ref("x1"));
        assert_eq!(clean_color(Some(&json!("#AABBCC"))), "#aabbcc");
        assert_eq!(clean_color(Some(&json!("red"))), "");
        assert!(time_id("g").starts_with('g'));
    }
}
