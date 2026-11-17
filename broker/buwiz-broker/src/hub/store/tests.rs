//! The store against recorded pins: a seeded database loads and serializes
//! byte for byte as pinned, and the start writes the pinned rows; what the
//! store writes reads back the same (R8); and the rules STORE-04 (building
//! turns) and STORE-05 (the uid backfill). The pins are under
//! `tests/pins/store/` (see `test-support/pins.rs`):
//!   seed_db            the seeded database, as SQL (`sqlite3 .dump`)
//!   seed_reads         what a read of it gives after the start
//!   seed_start_rows    its rows after that start
//!   rollback_reads     what a read gives of a database the store wrote
//!   rollback_rows      its rows after a save of that database
//! A row pin has `== <table>` and then the table's rows, one JSON object a
//! line, with what differs between any two runs of the same writes left
//! out (rowids, uids, timestamps, the data dir).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use regex_lite::Regex;
use serde_json::{json, Value};

use super::json::{out, threads_json, turns_json, Lookups};
use super::uuid7::v7_time;
use super::{Bot, Thread, Turn};
use crate::hub::ctx::HubCtx;
use crate::hub::library::botctx;
use crate::hub::traits::Clock;
use crate::pins;

fn temp(label: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("buwiz-store-{label}-{}", super::uuidv7()));
    std::fs::create_dir_all(d.join("widgets")).unwrap();
    d
}

fn rust_reads(ctx: &HubCtx) -> (Vec<(String, String)>, String) {
    let l = ctx.db.with(Lookups::read).unwrap();
    let gen = ctx.store.gen_dir();
    let turns = ctx.store.touch(|s| s.list.iter_mut().map(|t| (t.id.clone(), out(&turns_json(t, &l, &gen)))).collect());
    let bots = ctx.store.read(|s| s.bots.clone());
    let ints: Vec<(String, Value)> = ctx.db.with(|c| Ok(bots.iter().map(|b| (b.id.clone(), botctx::integrations_of(ctx, c, b))).collect())).unwrap();
    let threads = ctx.store.read(|s| out(&threads_json(s, "mock", |b| ints.iter().find(|x| x.0 == b.id).map(|x| x.1.clone()).unwrap_or(json!([])))));
    (turns, threads)
}

fn hub(dir: &Path) -> HubCtx {
    let ctx = HubCtx::new(dir.to_path_buf(), Clock::manual());
    ctx.store.load().unwrap();
    ctx
}

/// A pinned read: one `T <thread> <turnsJson>` line per thread and a
/// `THREADS <body>` line; `norm` writes uuids as `{uuid}`.
fn reads_text(turns: &[(String, String)], threads: &str, norm: bool) -> String {
    let mut text = String::new();
    for (id, json) in turns {
        text.push_str(&format!("T {id} {json}\n"));
    }
    text.push_str(&format!("THREADS {threads}\n"));
    if norm {
        text = Regex::new(UUID).unwrap().replace_all(&text, "{uuid}").into_owned();
    }
    text
}

/// A data dir whose database is the pinned seed.
fn seeded(label: &str) -> PathBuf {
    let dir = temp(label);
    std::fs::create_dir_all(dir.join("generated")).unwrap();
    let c = rusqlite::Connection::open(dir.join("generated/app.db")).unwrap();
    c.execute_batch(&pins::load_text("store/seed_db")).unwrap();
    dir
}

const UUID: &str = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";

/// The rows of `dir`'s database: per
/// table, a JSON line per row, sorted, after leaving out what differs
/// between any two runs of the same writes (turns.id, uids, timestamps,
/// every 13-digit millisecond time, the reply folders `m<base36>/`, bot
/// and group ids built from the clock, and the data dir).
fn db_rows(dir: &Path) -> BTreeMap<String, Vec<String>> {
    let c = rusqlite::Connection::open_with_flags(dir.join("generated/app.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let tables: Vec<String> = c
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .filter(|n: &String| n != "sqlite_sequence")
        .collect();
    let mut dirs = vec![dir.display().to_string()];
    if let Ok(real) = std::fs::canonicalize(dir) {
        dirs.push(real.display().to_string());
    }
    dirs.sort();
    dirs.dedup();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.len()));
    let (uuid, rel, ms, ided) = (Regex::new(UUID).unwrap(), Regex::new(r"\bm[0-9a-z]{6,10}/").unwrap(), Regex::new(r"\b1[0-9]{12}\b").unwrap(), Regex::new(r"\b[bg][0-9a-z]{8,12}\b").unwrap());
    let time_col = Regex::new(r"^(at|last_run|last_check|clock_in|clock_out)$|_at$").unwrap();
    let all = |t: &str| -> Vec<serde_json::Map<String, Value>> {
        let mut st = c.prepare(&format!("SELECT * FROM \"{t}\"")).unwrap();
        let names: Vec<String> = st.column_names().iter().map(|n| n.to_string()).collect();
        st.query_map([], |r| {
            let mut m = serde_json::Map::new();
            for (i, n) in names.iter().enumerate() {
                use rusqlite::types::ValueRef::*;
                m.insert(
                    n.clone(),
                    match r.get_ref(i)? {
                        Null => Value::Null,
                        Integer(v) => json!(v),
                        Real(v) if v.fract() == 0.0 && v.abs() < 1e15 => json!(v as i64),
                        Real(v) => json!(v),
                        Text(t) => json!(String::from_utf8_lossy(t)),
                        Blob(b) => json!(b),
                    },
                );
            }
            Ok(m)
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    };
    let id_of = |r: &serde_json::Map<String, Value>| r["id"].as_str().map(str::to_owned).unwrap_or_else(|| r["id"].to_string());
    // ids built from Date.now(), named by their order instead
    let mut ids = BTreeMap::new();
    for (table, prefix) in [("bots", "bot"), ("groups", "group")] {
        if tables.iter().any(|t| t == table) {
            let mut rows = all(table);
            rows.sort_by_key(|r| (r.get("ord").and_then(Value::as_i64).unwrap_or(0), id_of(r)));
            for (k, r) in rows.iter().enumerate() {
                ids.insert(id_of(r), format!("{prefix}#{k}"));
            }
        }
    }
    let norm = |s: &str| -> String {
        let mut s = s.to_owned();
        for d in &dirs {
            s = s.replace(d.as_str(), "{data}");
        }
        let s = uuid.replace_all(&s, "{uuid}").into_owned();
        let s = rel.replace_all(&s, "{rel}/").into_owned();
        let s = ms.replace_all(&s, "{ms}").into_owned();
        if ids.is_empty() {
            return s;
        }
        ided.replace_all(&s, |m: &regex_lite::Captures| ids.get(&m[0]).cloned().unwrap_or_else(|| m[0].to_owned())).into_owned()
    };
    tables
        .iter()
        .map(|t| {
            let mut rows: Vec<String> = all(t)
                .into_iter()
                .map(|row| {
                    let mut out = serde_json::Map::new();
                    for (k, v) in row {
                        if (t == "turns" && k == "id") || k == "uid" || time_col.is_match(&k) {
                            continue;
                        }
                        let v = match v {
                            Value::String(s) => json!(norm(&s)),
                            Value::Number(n) if n.as_f64().is_some_and(|f| (1e12..2e12).contains(&f)) => json!("{ms}"),
                            v => v,
                        };
                        out.insert(k, v);
                    }
                    Value::Object(out).to_string()
                })
                .collect();
            rows.sort();
            (t.clone(), rows)
        })
        .collect()
}

fn rows_text(rows: &BTreeMap<String, Vec<String>>) -> String {
    rows.iter().map(|(t, r)| format!("== {t}\n{}", r.iter().map(|l| format!("{l}\n")).collect::<String>())).collect()
}

/// `dir`'s rows against the pin, table by table, leaving out the tables only
/// one side has (the pin has other modules' tables, and the store
/// has tables of its own).
fn assert_rows(dir: &Path, pin: &str, what: &str) {
    let ours = db_rows(dir);
    let mut pinned: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut table = String::new();
    for line in pins::load_text(pin).lines() {
        if let Some(t) = line.strip_prefix("== ") {
            table = t.to_owned();
            pinned.entry(table.clone()).or_default();
        } else {
            pinned.get_mut(&table).unwrap().push(line.to_owned());
        }
    }
    let mut diffs = Vec::new();
    for (t, want) in &pinned {
        let Some(got) = ours.get(t) else { continue };
        let mut left = want.clone();
        left.sort();
        for row in got {
            match left.iter().position(|w| w == row) {
                Some(i) => {
                    left.remove(i);
                }
                None => diffs.push(format!("{t}: only in Rust: {row}")),
            }
        }
        diffs.extend(left.into_iter().map(|row| format!("{t}: only in the pin: {row}")));
    }
    if diffs.is_empty() {
        return;
    }
    // a refresh rewrites the pin's tables Rust has and leaves the others, then fails
    if pins::refreshing() {
        for (t, rows) in &mut pinned {
            if let Some(now) = ours.get(t) {
                *rows = now.clone();
            }
        }
        pins::write_text(pin, &rows_text(&pinned));
        pins::rewritten(pin, "txt");
    }
    panic!("{what}:\n{}", diffs.join("\n"));
}

#[test]
fn store_loads_a_node_seeded_database_and_reads_it_as_node_did() {
    let dir = seeded("seed");
    // the start (a load and a save) is the pins
    let ctx = hub(&dir);
    let (turns, threads) = rust_reads(&ctx);
    pins::check_text("store/seed_reads", &reads_text(&turns, &threads, false));
    assert_rows(&dir, "store/seed_start_rows", "the start's writes");
    // and a second load reads the same bytes
    let again = hub(&dir);
    assert_eq!(rust_reads(&again), (turns, threads));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn store_rust_writes_what_node_loaded_back_unchanged() {
    let dir = temp("rollback");
    let ctx = hub(&dir);
    let first = ctx.store.read(|s| s.list[0].id.clone());
    let (routine, _) = ctx.store.new_thread("Routine · Daily", false, |t| t.routine = Some(json!({"plugin": "p.one", "id": "daily", "name": "Daily", "thread": first}))).unwrap();
    let (agents, _) = ctx.store.new_thread("Q3", true, |t| t.agents = Some(json!({"subject": "Q3", "participants": ["ann"]}))).unwrap();
    ctx.store.append_turn(&first, Turn::user("hello").with_extra(&json!({"reply": {"i": 0, "text": "q"}, "attachments": [{"uri": "file:///a.png"}]}))).unwrap();
    let card = Turn { name: Some("R1".into()), v: Some(1), widget: true, art: Some(json!({"title": "Counter", "desc": "", "w": 0, "h": 0})), ..Turn::ai("", "mock") };
    let card = ctx.store.append_turn(&first, card).unwrap().unwrap();
    ctx.store.append_turn(&first, Turn { building: true, status: Some("checking".into()), ..Turn::ai("", "x") }).unwrap();
    ctx.store
        .append_turn(&first, Turn { links: Some(json!([{"provider": "gdrive", "id": "f", "name": "a.pdf", "account": "x"}])), suggest: Some(json!(["next"])), ..Turn::ai("done", "claude") })
        .unwrap();
    ctx.store.update_turn(&first, &card.turn, |t| t.reaction = Some("👍".into())).unwrap();
    ctx.store.append_turn(&first, Turn::notice("Daily ran", json!({"routine": "daily", "thread": routine}))).unwrap();
    ctx.store.append_turn(&routine, Turn::user("Routine · Daily").with_extra(&json!({"routine": {"plugin": "p.one", "id": "daily", "run": 1}}))).unwrap();
    ctx.store
        .append_turn(
            &agents,
            Turn { role: "agent_message".into(), message: Some(json!({"id": "m1", "to": ["ann"]})), pending: Some(json!(["ann"])), read_by: Some(json!([])), ..Turn::user("Check Q3.") },
        )
        .unwrap();
    ctx.store
        .write(|s| {
            let mut ann = Bot::new("ann", "Ann", "star", "#FF309B", Some(first.clone()));
            ann.crop = json!({"x": 1, "y": 2, "w": 3, "h": 4});
            ann.memories.push(json!({"id": "m", "text": "likes tea", "at": 1700000000000u64}));
            ann.made_by = Some(json!("bob"));
            ann.section = "Work".into();
            s.bots.push(ann);
            // a kit shape's stray outline is never written
            s.bots.push(Bot { description: None, instructions: None, outline: Some("M1 1 L9 1 L9 9 Z".into()), layers: Some(vec![json!({"path": "M1 1 L9 1 L9 9 Z", "fill": "#000000"})]), eyes: Some(json!("#FFFFFF")), ..Bot::new("bob", "Bob", "blob", "#1084FE", None) });
            s.bots.push(Bot {
                layers: Some(vec![json!({"path": "M34 64 C34 56 66 56 66 64 C66 76 58 82 50 82 C42 82 34 76 34 64 Z", "fill": "#FBEBD3"}), json!({"path": "M30 66 L38 66 L34 70 Z", "fill": "#F2C3B5", "opacity": 0.6}), json!({"path": "M30 40 L50 40 L40 56 Z", "fill": "#2B1D12", "over": true})]),
                parts: Some(vec![json!({"outline": "M12 38 L28 34 L34 10 L66 10 L72 34 L88 38 L88 44 L12 44 Z", "color": "#4C8CC9", "layers": [{"path": "M32 24 L68 24 L68 30 L32 30 Z", "fill": "#F4F4F4"}], "z": 1}), json!({"outline": "M40 30 L60 30 L60 50 L40 50 Z", "color": "#7A4A22", "z": -0.5, "d": 1.1})]),
                eyes: Some(json!({"c": "#2B1D12", "at": [50, 52.5], "gap": 21, "s": 1.1, "rim": "#111111", "rw": 0.25, "shut": "r"})),
                outline: Some("M18 44 L22 14 L42 28 Q50 26 58 28 L78 14 L82 44 C88 66 72 86 50 86 C28 86 12 66 18 44 Z".into()), ..Bot::new("cy", "Cy", "custom", "#FFFFFF", Some(agents.clone())) });
            s.sections = json!(["Work"]);
            s.active = routine.clone();
            let t = s.fresh_thread("moved");
            s.list.insert(1, t);
        })
        .unwrap();
    ctx.store.remove_turn(&first, &card.turn).unwrap();
    // the building card stays in memory only: a load reads what the store wrote
    // (the placeholder's gap closed, bob's NULL context read as the
    // defaults), and so does a Rust restart
    let (turns, threads) = rust_reads(&hub(&dir));
    pins::check_text("store/rollback_reads", &reads_text(&turns, &threads, true));
    assert!(threads.contains("\"description\":\"Bob is a personal assistant bot in Buwiz Bot.\""));
    assert!(threads.contains("\"shape\":\"custom\",\"color\":\"#FFFFFF\",\"outline\":\"M18 44 L22 14"), "a custom bot's outline, in bots[] and list[].bot");
    // its outline and its two parts', in bots[] and list[].bot: bob's stray outline never
    assert_eq!(threads.matches("\"outline\"").count(), 6, "only the custom bot's");
    assert!(threads.contains("\"layers\":[{\"path\":\"M34 64 C34 56"), "a rich bot's layers");
    assert_eq!(threads.matches("\"eyes\":{\"c\":\"#2B1D12\",\"at\":[50,52.5],\"gap\":21,\"s\":1.1,\"rim\":\"#111111\",\"rw\":0.25,\"shut\":\"r\"}").count(), 2, "a placement, in bots[] and list[].bot");
    assert_eq!(threads.matches("\"fill\":\"#2B1D12\",\"over\":true}").count(), 2, "an over-layer");
    // the custom bot's head layers and its hat's, in bots[] and list[].bot; bob's stray ones never
    assert_eq!(threads.matches("\"layers\"").count(), 4, "only the custom bot's");
    assert_eq!(threads.matches("\"parts\":[{\"outline\":\"M12 38").count(), 2, "its parts, in bots[] and list[].bot");
    assert_eq!(threads.matches("\"z\":-0.5,\"d\":1.1}").count(), 2, "a part's depth");
    assert_rows(&dir, "store/rollback_rows", "a save wrote what the store wrote, the placeholder's gap closed");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn store_building_turns_are_never_written_and_seq_keeps_the_gap() {
    let dir = temp("building");
    let ctx = hub(&dir);
    let t = ctx.store.read(|s| s.list[0].id.clone());
    ctx.store.append_turn(&t, Turn::user("build it")).unwrap();
    ctx.store.append_turn(&t, Turn { building: true, ..Turn::ai("", "x") }).unwrap();
    ctx.store.append_turn(&t, Turn::ai("done", "m")).unwrap();
    let rows: Vec<(i64, String)> = ctx
        .db
        .with(|c| {
            let mut st = c.prepare("SELECT seq, role FROM turns WHERE thread_id=? ORDER BY seq")?;
            let v = st.query_map([&t], |r| Ok((r.get(0)?, r.get(1)?)))?.collect();
            v
        })
        .unwrap();
    assert_eq!(rows, [(0, "user".to_owned()), (2, "ai".to_owned())]);
    let again = hub(&dir);
    assert_eq!(again.store.read(|s| s.thread(&t).unwrap().turns.len()), 2);
    let seqs: Vec<i64> = again.db.with(|c| c.prepare("SELECT seq FROM turns ORDER BY seq")?.query_map([], |r| r.get(0))?.collect()).unwrap();
    assert_eq!(seqs, [0, 1], "the start renumbers");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn store_backfills_uids_by_nodes_time_rule_once() {
    let dir = temp("uids");
    std::fs::create_dir_all(dir.join("generated")).unwrap();
    let k: i64 = 1_700_000_000_000;
    let c = rusqlite::Connection::open(dir.join("generated/app.db")).unwrap();
    c.execute_batch(&format!(
        "CREATE TABLE meta(k TEXT PRIMARY KEY, v TEXT);
         CREATE TABLE threads(id TEXT PRIMARY KEY, title TEXT NOT NULL, ord INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE turns(id INTEGER PRIMARY KEY AUTOINCREMENT, thread_id TEXT NOT NULL, seq INTEGER NOT NULL, role TEXT NOT NULL,
           text TEXT, name TEXT, label TEXT, caption TEXT, reaction TEXT, retries INTEGER DEFAULT 0, widget INTEGER DEFAULT 0);
         CREATE TABLE versions(name TEXT NOT NULL, v INTEGER NOT NULL, parent INTEGER, prompt TEXT, created_at INTEGER, PRIMARY KEY(name, v));
         INSERT INTO meta VALUES('active','t2');
         INSERT INTO threads VALUES('t2','Second',0),('t1','First',1);
         INSERT INTO turns(thread_id,seq,role,text,name,label) VALUES
           ('t1',0,'user','a',NULL,NULL),('t1',1,'ai',NULL,'R1','mock'),
           ('t2',0,'user','b',NULL,NULL),('t2',1,'ai','c',NULL,'claude');
         INSERT INTO versions VALUES('R1',1,NULL,'a',{k});"
    ))
    .unwrap();
    drop(c);
    let ids = |ctx: &HubCtx| {
        ctx.store.read(|s| {
            let t = |id: &str| s.thread(id).unwrap().clone();
            let (t1, t2): (Thread, Thread) = (t("t1"), t("t2"));
            [t1.uid.clone()].into_iter().chain(t1.turns.iter().map(|u| u.id.clone())).chain([t2.uid.clone()]).chain(t2.turns.iter().map(|u| u.id.clone())).map(Option::unwrap).collect::<Vec<_>>()
        })
    };
    let first = ids(&hub(&dir));
    assert_eq!(first.iter().map(|id| v7_time(Some(id)).unwrap()).collect::<Vec<_>>(), [k - 2, k - 1, k, k + 1, k + 2, k + 3]);
    assert_eq!(ids(&hub(&dir)), first, "idempotent");
    let _ = std::fs::remove_dir_all(dir);
}
