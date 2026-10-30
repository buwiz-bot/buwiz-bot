//! The store's writes (D33): the rows `persist()` would write, compared
//! with what was last written, and only the difference sent, in one
//! `BEGIN IMMEDIATE` transaction (so a write never deadlocks against the
//! leaf's `kv` and `timeclock` writes: STORE-07).

use std::collections::{HashMap, HashSet};

use rusqlite::{params_from_iter, Connection};
use serde_json::{json, Value};

use super::json::{sql_json, sql_text};
use super::uuid7::uuidv7;
use super::{State, Turn, Written};

pub type SqlValue = rusqlite::types::Value;
pub type Row = Vec<SqlValue>;

/// A column's value as node:sqlite hands it to JavaScript.
pub fn to_json(v: SqlValue) -> Value {
    match v {
        SqlValue::Null => Value::Null,
        SqlValue::Integer(i) => json!(i),
        SqlValue::Real(f) => serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null),
        SqlValue::Text(t) => Value::String(t),
        SqlValue::Blob(b) => Value::Object(b.iter().enumerate().map(|(i, byte)| (i.to_string(), json!(byte))).collect()),
    }
}

fn text(s: &str) -> SqlValue {
    SqlValue::Text(sql_text(s))
}

fn opt(s: &Option<String>) -> SqlValue {
    s.as_deref().map(text).unwrap_or(SqlValue::Null)
}

/// `x || null` of a string.
fn or_null(s: &str) -> SqlValue {
    if s.is_empty() {
        SqlValue::Null
    } else {
        text(s)
    }
}

fn int(i: i64) -> SqlValue {
    SqlValue::Integer(i)
}

/// Two column values as equal as JavaScript reads them (an INTEGER 2 and a
/// REAL 2.0 are both 2).
fn same(a: &SqlValue, b: &SqlValue) -> bool {
    match (a, b) {
        (SqlValue::Integer(i), SqlValue::Real(f)) | (SqlValue::Real(f), SqlValue::Integer(i)) => *i as f64 == *f,
        _ => a == b,
    }
}

fn same_row(a: &Row, b: &Row) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(x, y))
}

/// A thread's row without its ord: title, hidden, archived, uid, routine, agents.
pub fn thread_row(t: &super::Thread) -> Row {
    let mark = |v: &Option<Value>| v.as_ref().filter(|v| crate::js::truthy(v)).map(|v| SqlValue::Text(sql_json(v))).unwrap_or(SqlValue::Null);
    vec![text(&t.title), int(t.hidden as i64), int(t.archived as i64), opt(&t.uid), mark(&t.routine), mark(&t.agents)]
}

/// A turn's row: seq, role, text, name, label, caption, reaction, retries,
/// widget, meta, v, uid.
pub fn turn_row(seq: usize, u: &Turn) -> Row {
    vec![
        int(seq as i64),
        text(&u.role),
        opt(&u.text),
        opt(&u.name),
        opt(&u.label),
        opt(&u.caption),
        opt(&u.reaction),
        int(u.retries.unwrap_or(0)),
        int(u.widget as i64),
        u.meta_json().map(SqlValue::Text).unwrap_or(SqlValue::Null),
        u.v.map(int).unwrap_or(SqlValue::Null),
        opt(&u.id),
    ]
}

/// A bot's row: name, shape, color, thread_id, ord, pinned, section,
/// hidden, image, crop, model, effort, fast, label, description,
/// instructions, memories, integrations, extra.
pub fn bot_row(ord: usize, b: &super::Bot) -> Row {
    vec![
        text(&b.name),
        text(&b.shape),
        text(&b.color),
        opt(&b.thread_id),
        int(ord as i64),
        int(b.pinned as i64),
        or_null(&b.section),
        int(b.hidden as i64),
        or_null(&b.image),
        if crate::js::truthy(&b.crop) { SqlValue::Text(sql_json(&b.crop)) } else { SqlValue::Null },
        or_null(&b.model),
        or_null(&b.effort),
        int(b.fast as i64),
        or_null(&b.label),
        opt(&b.description),
        opt(&b.instructions),
        SqlValue::Text(sql_json(&Value::Array(b.memories.clone()))),
        SqlValue::Text(sql_json(&Value::Array(b.integrations.clone()))),
        b.extra_json().map(SqlValue::Text).unwrap_or(SqlValue::Null),
    ]
}

/// The rows `persist()` would write for `s`.
fn snapshot(s: &State) -> Written {
    let mut w = Written::default();
    for (i, t) in s.list.iter().enumerate() {
        w.threads.insert(t.id.clone(), thread_row(t));
        w.ord.insert(t.id.clone(), int(i as i64));
        let rows: Vec<Row> = t.turns.iter().enumerate().filter(|(_, u)| !u.building).map(|(j, u)| turn_row(j, u)).collect();
        w.turns.insert(t.id.clone(), rows);
    }
    for (k, b) in s.bots.iter().enumerate() {
        w.bots.insert(b.id.clone(), bot_row(k, b));
    }
    let sections = if crate::js::truthy(&s.sections) { s.sections.clone() } else { json!([]) };
    w.meta.insert("active", Some(sql_text(&s.active)));
    w.meta.insert("sections", Some(sql_json(&sections)));
    w.meta.insert("seq", Some(s.seq.to_string()));
    w
}

const TURN_INSERT: &str = "INSERT INTO turns(thread_id,seq,role,text,name,label,caption,reaction,retries,widget,meta,v,uid) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)";

/// Writes what changed since the last write, minting missing uids first
/// (as `persist()` does, threads then their turns in list order).
pub fn persist(conn: &Connection, s: &mut State) -> Result<(), String> {
    for t in s.list.iter_mut() {
        if t.uid.is_none() {
            t.uid = Some(uuidv7());
        }
        for u in t.turns.iter_mut().filter(|u| !u.building) {
            if u.id.is_none() {
                u.id = Some(uuidv7());
            }
        }
    }
    let want = snapshot(s);
    let old = &s.written;
    let mut sql: Vec<(&'static str, Row)> = Vec::new();
    let ids = |t: &str| vec![SqlValue::Text(t.to_owned())];
    // threads and their turns
    let gone: HashSet<&String> = old.threads.keys().chain(old.turns.keys()).filter(|id| !want.threads.contains_key(*id)).collect();
    for id in gone {
        sql.push(("DELETE FROM threads WHERE id=?", ids(id)));
        sql.push(("DELETE FROM turns WHERE thread_id=?", ids(id)));
    }
    for t in &s.list {
        let row = &want.threads[&t.id];
        let ord = &want.ord[&t.id];
        match old.threads.get(&t.id) {
            Some(r) if same_row(r, row) => {
                if !old.ord.get(&t.id).is_some_and(|o| same(o, ord)) {
                    sql.push(("UPDATE threads SET ord=? WHERE id=?", vec![ord.clone(), SqlValue::Text(t.id.clone())]));
                }
            }
            _ => {
                let mut r = vec![SqlValue::Text(t.id.clone()), row[0].clone(), ord.clone()];
                r.extend(row[1..].iter().cloned());
                sql.push(("INSERT OR REPLACE INTO threads(id,title,ord,hidden,archived,uid,routine,agents) VALUES(?,?,?,?,?,?,?,?)", r));
            }
        }
        let new = &want.turns[&t.id];
        let had = old.turns.get(&t.id).map(Vec::as_slice).unwrap_or(&[]);
        let prefix = had.len() <= new.len() && had.iter().zip(new).all(|(a, b)| same_row(a, b));
        let from = if prefix {
            had.len()
        } else {
            sql.push(("DELETE FROM turns WHERE thread_id=?", ids(&t.id)));
            0
        };
        for r in &new[from..] {
            let mut r2 = vec![SqlValue::Text(t.id.clone())];
            r2.extend(r.iter().cloned());
            sql.push((TURN_INSERT, r2));
        }
    }
    // bots, row by row
    for id in old.bots.keys().filter(|id| !want.bots.contains_key(*id)) {
        sql.push(("DELETE FROM bots WHERE id=?", ids(id)));
    }
    for b in &s.bots {
        let row = &want.bots[&b.id];
        if !old.bots.get(&b.id).is_some_and(|r| same_row(r, row)) {
            let mut r = vec![SqlValue::Text(b.id.clone())];
            r.extend(row.iter().cloned());
            sql.push(("INSERT OR REPLACE INTO bots(id,name,shape,color,thread_id,ord,pinned,section,hidden,image,crop,model,effort,fast,label,description,instructions,memories,integrations,extra) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)", r));
        }
    }
    for k in ["active", "sections", "seq"] {
        let v = want.meta.get(k).cloned().flatten();
        if old.meta.get(k).cloned().flatten() != v {
            sql.push(("INSERT OR REPLACE INTO meta(k,v) VALUES(?,?)", vec![SqlValue::Text(k.into()), v.map(SqlValue::Text).unwrap_or(SqlValue::Null)]));
        }
    }
    if sql.is_empty() {
        s.written = want;
        return Ok(());
    }
    conn.execute_batch("BEGIN IMMEDIATE").map_err(|e| e.to_string())?;
    let run = || -> rusqlite::Result<()> {
        for (q, args) in &sql {
            conn.prepare_cached(q)?.execute(params_from_iter(args.iter()))?;
        }
        Ok(())
    };
    match run() {
        Ok(()) => {
            conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
            s.written = want;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e.to_string())
        }
    }
}

/// A `meta` value as text (`None` for no row or NULL).
pub fn meta_text(v: Option<SqlValue>) -> Option<String> {
    match v? {
        SqlValue::Null => None,
        SqlValue::Text(t) => Some(t),
        other => Some(crate::js::string_of(&to_json(other))),
    }
}

/// What is in app.db now, as [`Written`] (the load's starting point).
pub(crate) fn read_written(conn: &Connection) -> rusqlite::Result<Written> {
    let mut w = Written::default();
    let mut st = conn.prepare("SELECT id,title,ord,hidden,archived,uid,routine,agents FROM threads")?;
    let rows = st.query_map([], |r| {
        let v: Row = (0..8).map(|i| r.get::<_, SqlValue>(i)).collect::<rusqlite::Result<_>>()?;
        Ok(v)
    })?;
    for r in rows {
        let r = r?;
        let id = match &r[0] {
            SqlValue::Text(t) => t.clone(),
            other => crate::js::string_of(&to_json(other.clone())),
        };
        w.ord.insert(id.clone(), r[2].clone());
        w.threads.insert(id, vec![r[1].clone(), r[3].clone(), r[4].clone(), r[5].clone(), r[6].clone(), r[7].clone()]);
    }
    let mut st = conn.prepare("SELECT thread_id,seq,role,text,name,label,caption,reaction,retries,widget,meta,v,uid FROM turns ORDER BY seq, id")?;
    let rows = st.query_map([], |r| {
        let v: Row = (0..13).map(|i| r.get::<_, SqlValue>(i)).collect::<rusqlite::Result<_>>()?;
        Ok(v)
    })?;
    let mut turns: HashMap<String, Vec<Row>> = HashMap::new();
    for r in rows {
        let r = r?;
        let id = crate::js::string_of(&to_json(r[0].clone()));
        turns.entry(id).or_default().push(r[1..].to_vec());
    }
    w.turns = turns;
    let mut st = conn.prepare("SELECT id,name,shape,color,thread_id,ord,pinned,section,hidden,image,crop,model,effort,fast,label,description,instructions,memories,integrations,extra FROM bots")?;
    let rows = st.query_map([], |r| {
        let v: Row = (0..20).map(|i| r.get::<_, SqlValue>(i)).collect::<rusqlite::Result<_>>()?;
        Ok(v)
    })?;
    for r in rows {
        let r = r?;
        w.bots.insert(crate::js::string_of(&to_json(r[0].clone())), r[1..].to_vec());
    }
    for k in ["active", "sections", "seq"] {
        let v: Option<SqlValue> = conn.query_row("SELECT v FROM meta WHERE k=?", [k], |r| r.get(0)).map(Some).or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(e) })?;
        w.meta.insert(k, meta_text(v));
    }
    Ok(w)
}
