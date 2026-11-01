//! The store read from app.db as state.mjs's `loadStore` reads it, the
//! first boot's legacy import (generated/threads.json, turns.json), the
//! counters (`seq` against the `R<n>.tsx` files on disk, `threadSeq`), and
//! `backfillIds`' time rule for rows written before uids.

use std::path::Path;

use rusqlite::Connection;
use serde_json::{json, Map, Value};

use super::json::{bot_from, bot_set, js_number_str, turn_from};
use super::uuid7::{now_ms, uuidv7, uuidv7_at, v7_time};
use super::write::{read_written, to_json, SqlValue};
use super::{bot_defaults, Bot, Ceiling, State, Thread};
use crate::js;

fn col(r: &rusqlite::Row, name: &str) -> rusqlite::Result<Value> {
    Ok(to_json(r.get::<_, SqlValue>(name)?))
}

fn parse(text: &Value, what: &str) -> Option<Value> {
    let s = text.as_str()?;
    match serde_json::from_str(s) {
        Ok(v) => Some(v),
        Err(e) => {
            eprintln!("rox-broker: the store: {what} is not JSON ({e}); left out");
            None
        }
    }
}

/// The store as Node loads it at start.
pub fn load(conn: &Connection, gen: &Path) -> Result<State, String> {
    load_rows(conn, gen).map_err(|e| e.to_string())
}

fn load_rows(conn: &Connection, gen: &Path) -> rusqlite::Result<State> {
    let written = read_written(conn)?;
    let meta = |k: &str| written.meta.get(k).cloned().flatten();
    let mut s = State { active: meta("active").filter(|a| !a.is_empty()).unwrap_or_else(|| "t1".into()), ..Default::default() };
    let stored_seq = js_number_str(&meta("seq").filter(|x| !x.is_empty()).unwrap_or_else(|| "0".into()));

    let mut st = conn.prepare("SELECT * FROM threads ORDER BY ord")?;
    let threads: Vec<(Value, Value, Value, Value, Value, Value, Value)> =
        st.query_map([], |r| Ok((col(r, "id")?, col(r, "uid")?, col(r, "title")?, col(r, "hidden")?, col(r, "archived")?, col(r, "routine")?, col(r, "agents")?)))?.collect::<rusqlite::Result<_>>()?;
    let mut turns_of = conn.prepare("SELECT * FROM turns WHERE thread_id=? ORDER BY seq, id")?;
    for (id, uid, title, hidden, archived, routine, agents) in threads {
        let id = js::string_of(&id);
        let mut t = Thread {
            id: id.clone(),
            uid: uid.as_str().filter(|u| !u.is_empty()).map(str::to_owned),
            title: js::string_of(&title),
            hidden: js::truthy(&hidden),
            archived: js::truthy(&archived),
            routine: if js::truthy(&routine) { parse(&routine, "a thread's routine") } else { None },
            agents: if js::truthy(&agents) { parse(&agents, "a thread's agents") } else { None },
            turns: Vec::new(),
        };
        let rows: Vec<Map<String, Value>> = turns_of
            .query_map([&id], |r| {
                let mut m = Map::new();
                for name in ["role", "uid", "text", "name", "label", "caption", "reaction", "retries", "widget", "meta", "v"] {
                    m.insert(name.into(), col(r, name)?);
                }
                Ok(m)
            })?
            .collect::<rusqlite::Result<_>>()?;
        for u in rows {
            let role = js::string_of(&u["role"]);
            let mut o = Map::new();
            // BUG: a user message's reaction is written (turns.reaction) but
            // never loaded back: user, notice and agent_message rows read only
            // role, uid, text and meta, so the badge is gone after a restart
            // and the start's write sets the column back to NULL (STORE-01)
            let plain = matches!(role.as_str(), "user" | "notice" | "agent_message");
            o.insert("role".into(), json!(if plain { role.as_str() } else { "ai" }));
            if js::truthy(&u["uid"]) {
                o.insert("id".into(), u["uid"].clone());
            }
            o.insert("text".into(), u["text"].clone());
            if !plain {
                let or = |k: &str, d: Value| if js::truthy(&u[k]) { u[k].clone() } else { d };
                o.insert("name".into(), u["name"].clone());
                o.insert("label".into(), or("label", json!("")));
                o.insert("caption".into(), or("caption", json!("")));
                o.insert("reaction".into(), or("reaction", json!("")));
                o.insert("retries".into(), or("retries", json!(0)));
                o.insert("v".into(), u["v"].clone());
                o.insert("widget".into(), json!(js::truthy(&u["widget"])));
            }
            if js::truthy(&u["meta"]) {
                if let Some(Value::Object(m)) = parse(&u["meta"], "a turn's meta") {
                    o.extend(m);
                }
            }
            t.turns.push(turn_from(&Value::Object(o)));
        }
        s.list.push(t);
    }

    let mut st = conn.prepare("SELECT * FROM bots ORDER BY ord")?;
    let names: Vec<String> = st.column_names().into_iter().map(str::to_owned).collect();
    let rows: Vec<Map<String, Value>> = st
        .query_map([], |r| {
            let mut m = Map::new();
            for n in &names {
                m.insert(n.clone(), col(r, n)?);
            }
            Ok(m)
        })?
        .collect::<rusqlite::Result<_>>()?;
    for b in rows {
        let get = |k: &str| b.get(k).cloned().unwrap_or(Value::Null);
        let s_or = |k: &str| if js::truthy(&get(k)) { js::string_of(&get(k)) } else { String::new() };
        let name = js::string_of(&get("name"));
        let (description, instructions) = bot_defaults(&name);
        let list = |k: &str| match get(k).as_str().map(serde_json::from_str::<Value>) {
            Some(Ok(Value::Array(a))) => a,
            _ => Vec::new(),
        };
        let mut bot = Bot {
            id: js::string_of(&get("id")),
            name,
            shape: js::string_of(&get("shape")),
            color: js::string_of(&get("color")),
            thread_id: get("thread_id").as_str().map(str::to_owned),
            pinned: js::truthy(&get("pinned")),
            section: s_or("section"),
            hidden: js::truthy(&get("hidden")),
            image: s_or("image"),
            crop: if js::truthy(&get("crop")) { parse(&get("crop"), "a bot's crop").unwrap_or(Value::Null) } else { Value::Null },
            model: s_or("model"),
            effort: s_or("effort"),
            fast: js::truthy(&get("fast")),
            label: s_or("label"),
            description: Some(get("description").as_str().map(str::to_owned).unwrap_or(description)),
            instructions: Some(get("instructions").as_str().map(str::to_owned).unwrap_or(instructions)),
            memories: list("memories"),
            integrations: list("integrations"),
            tools: parse(&get("tools"), "a bot's tools").map(|v| Ceiling::from_json(&v)).unwrap_or_default(),
            made_by: None,
            purpose: None,
            helper: None,
            accepts: None,
            updated_at: None,
        };
        if js::truthy(&get("extra")) {
            if let Some(Value::Object(x)) = parse(&get("extra"), "a bot's extra") {
                for (k, v) in x {
                    bot_set(&mut bot, &k, v);
                }
            }
        }
        s.bots.push(bot);
    }
    s.sections = meta("sections").and_then(|x| serde_json::from_str(&x).ok()).unwrap_or(json!([]));

    // first boot: the JSON files the older server wrote, else a first chat
    let mut seq = stored_seq;
    if s.list.is_empty() {
        let legacy = gen.join("threads.json");
        let old: Option<Value> = legacy.exists().then(|| std::fs::read_to_string(&legacy).ok().and_then(|t| serde_json::from_str(&t).ok())).flatten();
        s = State { written, ..Default::default() };
        match old {
            Some(old) => {
                eprintln!("migrated threads.json → app.db ({} threads)", old.get("list").and_then(Value::as_array).map(Vec::len).unwrap_or(0));
                s.active = old.get("active").filter(|a| js::truthy(a)).map(js::string_of).unwrap_or_else(|| "t1".into());
                seq = old.get("seq").and_then(Value::as_f64).unwrap_or(0.0);
                s.sections = old.get("sections").cloned().unwrap_or(json!([]));
                for t in old.get("list").and_then(Value::as_array).cloned().unwrap_or_default() {
                    let get = |k: &str| t.get(k).cloned().unwrap_or(Value::Null);
                    s.list.push(Thread {
                        id: js::string_of(&get("id")),
                        uid: get("uid").as_str().filter(|u| !u.is_empty()).map(str::to_owned),
                        title: js::string_of(&get("title")),
                        hidden: js::truthy(&get("hidden")),
                        archived: js::truthy(&get("archived")),
                        routine: Some(get("routine")).filter(js::truthy),
                        agents: Some(get("agents")).filter(js::truthy),
                        turns: get("turns").as_array().map(|a| a.iter().map(turn_from).collect()).unwrap_or_default(),
                    });
                }
                s.bots = old.get("bots").and_then(Value::as_array).map(|a| a.iter().map(bot_from).collect()).unwrap_or_default();
            }
            None => {
                let turns_file = gen.join("turns.json");
                let turns = turns_file
                    .exists()
                    .then(|| std::fs::read_to_string(&turns_file).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()))
                    .flatten()
                    .and_then(|v| v.as_array().map(|a| a.iter().map(turn_from).collect()))
                    .unwrap_or_default();
                s.active = "t1".into();
                seq = 0.0;
                s.sections = Value::Null;
                s.list.push(Thread { id: "t1".into(), uid: Some(uuidv7()), title: "first chat".into(), turns, ..Default::default() });
            }
        }
    } else {
        s.written = written;
    }

    // module names never collide with files already on disk
    let on_disk = std::fs::read_dir(gen).map(|d| d.filter_map(|e| e.ok()).filter_map(|e| module_number(&e.file_name().to_string_lossy())).max().unwrap_or(0)).unwrap_or(0);
    let seq = if seq.is_nan() { 0.0 } else { seq };
    s.seq = (seq.max(0.0) as u64).max(on_disk);
    s.thread_seq = s.list.iter().map(|t| thread_number(&t.id)).max().unwrap_or(0) + 1;
    backfill(conn, &mut s)?;
    Ok(s)
}

/// `R<n>.tsx` → n.
pub fn module_number(f: &str) -> Option<u64> {
    let n = f.strip_prefix('R')?.strip_suffix(".tsx")?;
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| n.parse().ok()).flatten()
}

/// `Number(t.id.slice(1)) || 0`.
fn thread_number(id: &str) -> u64 {
    let n = js_number_str(id.get(1..).unwrap_or(""));
    if n.is_finite() && n > 0.0 {
        n as u64
    } else {
        0
    }
}

/// state.mjs `backfillIds`: uids for rows written before they existed. A
/// known time stays; an artifact turn takes its version's `created_at`;
/// the rest fall in creation order (thread number, then turn order) right
/// after the previous known time.
fn backfill(conn: &Connection, s: &mut State) -> rusqlite::Result<()> {
    if !s.list.iter().any(|t| t.uid.is_none() || t.turns.iter().any(|u| u.id.is_none())) {
        return Ok(());
    }
    let mut vt: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    let mut st = conn.prepare("SELECT name,v,created_at FROM versions WHERE created_at IS NOT NULL")?;
    let rows: Vec<(Value, Value, Value)> = st.query_map([], |r| Ok((col(r, "name")?, col(r, "v")?, col(r, "created_at")?)))?.collect::<rusqlite::Result<_>>()?;
    for (name, v, at) in rows {
        let ms = super::json::js_number(Some(&at));
        if !ms.is_finite() || ms <= 0.0 {
            continue;
        }
        let name = js::string_of(&name);
        vt.insert(format!("{name}@{}", js::string_of(&v)), ms);
        if !vt.get(&name).is_some_and(|x| *x <= ms) {
            vt.insert(name, ms);
        }
    }
    let mut order: Vec<usize> = (0..s.list.len()).collect();
    order.sort_by_key(|&i| thread_number(&s.list[i].id));
    // (thread index, turn index or None for the thread), its known time
    let mut items: Vec<(usize, Option<usize>, Option<f64>)> = Vec::new();
    for &i in &order {
        let t = &s.list[i];
        items.push((i, None, v7_time(t.uid.as_deref()).map(|x| x as f64)));
        for (j, u) in t.turns.iter().enumerate() {
            let named = u.name.as_deref().filter(|n| !n.is_empty());
            let known = v7_time(u.id.as_deref())
                .map(|x| x as f64)
                .or_else(|| named.and_then(|n| vt.get(&format!("{n}@{}", u.v.map(|v| v.to_string()).unwrap_or_else(|| "null".into()))).or_else(|| vt.get(n)).copied()));
            items.push((i, Some(j), known));
        }
    }
    let first = items.iter().position(|x| x.2.is_some());
    let mut prev = match first {
        None => now_ms() as f64 - items.len() as f64 - 1.0,
        Some(k) => items[k].2.unwrap_or(0.0) - k as f64 - 1.0,
    };
    for (i, j, known) in items {
        prev = match known {
            Some(k) if k > prev => k,
            _ => prev + 1.0,
        };
        let at = prev.floor() as i64;
        match j {
            None => {
                let t = &mut s.list[i];
                if t.uid.is_none() {
                    t.uid = Some(uuidv7_at(at));
                }
            }
            Some(j) => {
                let u = &mut s.list[i].turns[j];
                if u.id.is_none() {
                    u.id = Some(uuidv7_at(at));
                }
            }
        }
    }
    Ok(())
}
