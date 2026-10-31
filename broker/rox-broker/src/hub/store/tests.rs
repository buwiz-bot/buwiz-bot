//! The store against Node's own modules (server/src/state.mjs): a
//! database Node seeded loads and serializes as Node's does, byte for
//! byte, and writes the rows Node's start would (`tools/db-diff.mjs`);
//! what Rust writes, Node loads into the same reads (R8); and the rules
//! Node's tests pin (STORE-04 building turns, STORE-05 the uid backfill).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

use super::json::{out, threads_json, turns_json, Lookups};
use super::uuid7::v7_time;
use super::{Bot, Thread, Turn};
use crate::hub::ctx::HubCtx;
use crate::hub::library::botctx;
use crate::hub::traits::Clock;

fn server() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../server")
}

fn temp(label: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("rox-store-{label}-{}", super::uuidv7()));
    std::fs::create_dir_all(d.join("widgets")).unwrap();
    d
}

/// `nodeIn`: an ES module body run by Node on data dir `dir`, cwd server/.
fn node_in(dir: &Path, code: &str) -> String {
    let o = Command::new("node")
        .args(["--input-type=module", "-e", &format!("{code}\nprocess.exit(0);")])
        .current_dir(server())
        .env("GROK_DATA_DIR", dir)
        .env("MOCK", "1")
        .env_remove("ROX_NODE_ROLE")
        .output()
        .expect("node runs");
    assert!(o.status.success(), "node failed: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// What Node reads back after a load and a `persist()`: each thread's
/// `turnsJson` and `/threads`' body, one per line.
const NODE_READS: &str = r#"
  const st = await import("./src/state.mjs");
  st.persist();
  const h = (await import("./src/routes/chat.mjs")).default;
  let body = "";
  await h({ method: "GET", url: "/threads", headers: {} }, { setHeader() {}, end(b) { body = b; } });
  for (const t of st.store.list) console.log("T " + t.id + " " + JSON.stringify(st.turnsJson(t)));
  console.log("THREADS " + body);
"#;

fn node_reads(dir: &Path) -> (Vec<(String, String)>, String) {
    let text = node_in(dir, NODE_READS);
    let mut turns = Vec::new();
    let mut threads = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("T ") {
            let (id, json) = rest.split_once(' ').unwrap();
            turns.push((id.to_owned(), json.to_owned()));
        } else if let Some(rest) = line.strip_prefix("THREADS ") {
            threads = rest.to_owned();
        }
    }
    (turns, threads)
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

fn copy_db(from: &Path, to: &Path) {
    std::fs::create_dir_all(to.join("generated")).unwrap();
    std::fs::create_dir_all(to.join("widgets")).unwrap();
    std::fs::copy(from.join("generated/app.db"), to.join("generated/app.db")).unwrap();
}

/// `tools/db-diff.mjs a b`'s differences in rows, leaving out tables only
/// one side has (the twin's Node imports other tracks' modules, which
/// create their own tables).
fn db_diff(a: &Path, b: &Path) -> String {
    let o = Command::new("node").arg("tools/db-diff.mjs").arg(a.join("generated/app.db")).arg(b.join("generated/app.db")).current_dir(server()).output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    let tables_only = text.lines().filter(|l| l.ends_with("(table only here)")).count();
    let rest: Vec<&str> = text.lines().filter(|l| !l.is_empty() && !l.ends_with("(table only here)") && *l != format!("{tables_only} rows differ")).collect();
    rest.join("\n")
}

const SEED: &str = r##"
  const { store, nextThreadSeq, uuidv7, writeIndex } = await import("./src/state.mjs");
  const t1 = store.list[0];
  const t2 = { id: "t" + nextThreadSeq(), uid: uuidv7(), title: "Routine thread", hidden: true, archived: true, routine: { plugin: "p.one", id: "daily", name: "Daily", thread: t1.id }, turns: [] };
  const t3 = { id: "t" + nextThreadSeq(), uid: uuidv7(), title: "Q3 😀 plans", agents: { subject: "Q3", participants: ["ann", "bob"] }, turns: [] };
  store.list.push(t2, t3);
  t1.turns.push(
    { id: uuidv7(), role: "user", text: "hello", reaction: "🎉", reply: { i: 0, text: "quoted", files: ["m1/a.txt"] }, attachments: [{ uri: "file:///x/a.png", name: "a.png", kind: "image" }], v1Attachments: [{ plugin: "p.one", token: "@p:x:1" }] },
    { id: uuidv7(), role: "ai", text: "answer", name: "", label: "claude-haiku-5-5", caption: "cap", reaction: "👍", retries: 2, files: [{ path: "m1/a.txt", name: "a.txt", kind: "text" }], links: [{ provider: "gdrive", id: "f1", name: "Doc", link: "https://example.com/d", mime: "application/vnd.google-apps.document", account: "a@example.com" }, { provider: "mcp-app", id: "mcp:apps/x", name: "X", token: "@apps:i:1", kind: "app", file_type: "An item" }], suggest: ["Next step"], art: { title: "T", desc: "D", w: 100, h: 80 } },
    { id: uuidv7(), role: "ai", text: null, name: "R1", label: "mock", v: 1, widget: true, art: { title: "Counter", desc: "", w: 0, h: 0 } },
    { id: uuidv7(), role: "notice", text: "Daily ran", notice: { routine: "daily", thread: t2.id } },
    { id: uuidv7(), role: "ai", text: "x".repeat(150) + " **bold** ## heading _under_ `code` > quote 😀😀", label: "m" },
  );
  t2.turns.push({ id: uuidv7(), role: "user", text: "Routine · Daily", routine: { plugin: "p.one", id: "daily", run: 1 } });
  t3.turns.push(
    { id: uuidv7(), role: "agent_message", text: "Check Q3.", message: { id: "m1", from: { id: "ann", name: "Ann" }, to: ["bob"], subject: "Q3" }, pending: ["bob"], read_by: ["ann"] },
    { id: uuidv7(), role: "ai", text: "woken", agent: { id: "bob", name: "Bob" } },
  );
  store.bots.push(
    { id: "ann", name: "Ann", shape: "star", color: "#FF309B", threadId: t1.id, pinned: true, section: "Work", hidden: false, image: "file:///x/ann.png", crop: { x: 0.1, y: 0.2, w: 0.5, h: 0.5 }, model: "gpt-5.5", effort: "high", fast: true, label: "tagline", description: "", instructions: "Be kind.", memories: [{ id: uuidv7(), text: "likes tea", at: 1700000000000 }], integrations: [{ id: "s1", kind: "skill", at: 1700000000000, via: "user" }], made_by: "bob", purpose: "help Bob", accepts: ["bob"] },
    { id: "bob", name: "Bob", shape: "blob", color: "#1084FE", threadId: null, helper: { parent: "ann", task: "sort" } },
  );
  store.sections = ["Work"];
  writeIndex();
"##;

#[test]
fn store_loads_a_node_seeded_database_and_reads_it_as_node_does() {
    let dir = temp("seed");
    node_in(&dir, SEED);
    let twin = temp("seed-twin");
    copy_db(&dir, &twin);
    // Node's own start (a load and a persist), and Rust's
    let (node_turns, node_threads) = node_reads(&twin);
    let ctx = hub(&dir);
    let (turns, threads) = rust_reads(&ctx);
    assert_eq!(turns, node_turns);
    assert_eq!(threads, node_threads);
    assert_eq!(db_diff(&twin, &dir), "", "the start's writes");
    // and a second load reads the same bytes
    let again = hub(&dir);
    assert_eq!(rust_reads(&again), (turns, threads));
    for d in [dir, twin] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn store_rust_writes_what_node_loads_back_unchanged() {
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
            s.bots.push(Bot { description: None, instructions: None, ..Bot::new("bob", "Bob", "blob", "#1084FE", None) });
            s.sections = json!(["Work"]);
            s.active = routine.clone();
            let t = s.fresh_thread("moved");
            s.list.insert(1, t);
        })
        .unwrap();
    ctx.store.remove_turn(&first, &card.turn).unwrap();
    // the building card stays in memory only: Node loads what Rust wrote
    // (the placeholder's gap closed, bob's NULL context read as the
    // defaults), and so does a Rust restart
    let twin = temp("rollback-twin");
    copy_db(&dir, &twin);
    let (node_turns, node_threads) = node_reads(&twin);
    let (turns, threads) = rust_reads(&hub(&dir));
    assert_eq!(turns, node_turns);
    assert_eq!(threads, node_threads);
    assert!(node_threads.contains("\"description\":\"Bob is a personal assistant bot in Buwiz Bot.\""));
    assert_eq!(db_diff(&twin, &dir), "", "Node's persist() writes what Rust wrote, the placeholder's gap closed");
    for d in [dir, twin] {
        let _ = std::fs::remove_dir_all(d);
    }
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
