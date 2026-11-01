//! The store (plan 5.1): threads, turns, bots and `meta`'s `active`,
//! `sections` and `seq`, held in memory as Node's state.mjs holds them and
//! written to app.db row by row (D33), never as `persist()`'s whole-store
//! rewrite.
//!
//! # The API the other tracks call
//!
//! The store is `ctx.store` (a [`Store`]). Everything goes through three
//! calls; none may be held across an `.await`:
//!
//! - [`Store::read`]`(|s: &State| …)`: a read.
//! - [`Store::write`]`(|s: &mut State| …)`: a change, persisted when the
//!   closure returns. Only what changed is written, each call one
//!   transaction: a changed thread's row and its turns (a turn appended at
//!   the end is one INSERT, any other change rewrites that thread's
//!   turns), a changed bot's row, `ord` renumbered when the list order
//!   changed, `meta` `active`/`sections`/`seq` when they changed. Missing
//!   uids (threads' `uid`, turns' `id`) are minted first, as `persist()`
//!   mints them. `Err` is the SQLite error; memory keeps the change and the
//!   next write retries it (Node's `persist()` throwing).
//! - [`Store::touch`]`(|s: &mut State| …)`: a change to what is never
//!   persisted (a building placeholder's `status`, `revising`).
//!
//! On [`State`]: [`State::thread`] / [`State::thread_mut`] (a `t<n>` id or
//! a uid, `findThread`), [`State::active_thread`], [`State::thread_bot`]
//! (`threadBot` without `actingBot`: a turn's `TurnCtx::acting_bot` comes
//! first), [`State::bot`] / [`State::bot_mut`], [`State::new_thread_id`]
//! (`t${nextThreadSeq()}`), [`State::next_module_seq`] (`nextSeq()`, the
//! `R<n>` counter, persisted as `meta.seq`), [`State::turns_json`] is
//! [`json::turns_json`].
//!
//! Turns are [`Turn`]s ([`Turn::user`], [`Turn::ai`], [`Turn::notice`]
//! build them; `meta` fields are `serde_json::Value`s). Shortcuts over
//! `write`: [`Store::append_turn`] (`thread.turns.push(…)` then
//! `writeIndex()`; mints the turn's id, answers its index and id),
//! [`Store::update_turn`] (by turn id), [`Store::remove_turn`],
//! [`Store::new_thread`] (a thread first or last in the list, uid minted,
//! its `routine` / `agents` mark set by the closure), [`Store::set_marks`]
//! (a thread's `routine` and `agents` marks).
//!
//! A building placeholder (`Turn::building`) lives in memory only: it is
//! never written, and its index still counts (`turns.seq` keeps the gap,
//! as Node writes it). [`crate::hub::jobs::Jobs::set`] drops it when its
//! job ends.
//!
//! Files: [`Store::write_index`] (`writeIndex()`'s generated/index.tsx,
//! rewritten only when the set of modules changes) and
//! [`Store::write_shelf`] (widgets/index.tsx, the shelf template as is).
//!
//! JSON: [`json::threads_json`] and [`json::turns_json`] are `/threads` and
//! `/turns`'s bodies; [`json::out`] serializes as `JSON.stringify`, lone
//! UTF-16 halves included ([`json::js_slice`] cuts as `s.slice(0, n)`).

pub mod index_tsx;
pub mod json;
pub mod load;
pub mod schema;
#[cfg(test)]
mod tests;
pub mod uuid7;
pub mod write;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

use super::ctx::Db;
use super::traits::TurnDone;
pub use uuid7::{now_ms, uuidv7};

/// A turn as Node keeps it in a thread (`{role, id, text, …}`). `None` is
/// JavaScript's `undefined`; a meta field read back as `null` is
/// `Some(Value::Null)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Turn {
    /// "user", "ai", "notice", "agent_message"
    pub role: String,
    /// the turn's permanent uid (Node's `id`)
    pub id: Option<String>,
    pub text: Option<String>,
    pub name: Option<String>,
    /// a card row loaded with a NULL name: Node's `null`, not `undefined`
    /// (only `/share/thread` and `/react` tell them apart)
    pub null_name: bool,
    pub label: Option<String>,
    pub caption: Option<String>,
    pub reaction: Option<String>,
    pub retries: Option<i64>,
    pub v: Option<i64>,
    pub widget: bool,
    pub reply: Option<Value>,
    pub attachments: Option<Value>,
    pub files: Option<Value>,
    pub art: Option<Value>,
    pub links: Option<Value>,
    pub suggest: Option<Value>,
    pub v1_attachments: Option<Value>,
    pub routine: Option<Value>,
    pub notice: Option<Value>,
    pub message: Option<Value>,
    pub pending: Option<Value>,
    pub read_by: Option<Value>,
    pub agent: Option<Value>,
    /// a card still being made: in memory only, never written
    pub building: bool,
    pub status: Option<String>,
    pub revising: bool,
}

impl Turn {
    /// `{id: uuidv7(), role: "user", text}`.
    pub fn user(text: &str) -> Self {
        Turn { role: "user".into(), id: Some(uuidv7()), text: Some(text.into()), ..Default::default() }
    }

    /// `{id: uuidv7(), role: "ai", text, label}`.
    pub fn ai(text: &str, label: &str) -> Self {
        Turn { role: "ai".into(), id: Some(uuidv7()), text: Some(text.into()), label: Some(label.into()), ..Default::default() }
    }

    /// `{id: uuidv7(), role: "notice", text, notice}`.
    pub fn notice(text: &str, notice: Value) -> Self {
        Turn { role: "notice".into(), id: Some(uuidv7()), text: Some(text.into()), notice: Some(notice), ..Default::default() }
    }
}

/// A chat (`store.list[i]`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Thread {
    /// "t<n>"
    pub id: String,
    pub uid: Option<String>,
    pub title: String,
    pub hidden: bool,
    pub archived: bool,
    /// a v1 routine's own thread (v1/routine-threads.mjs)
    pub routine: Option<Value>,
    /// an agents' conversation (agents.mjs)
    pub agents: Option<Value>,
    pub turns: Vec<Turn>,
}

impl Thread {
    /// The index and the turn whose id is `uid`.
    pub fn turn(&self, uid: &str) -> Option<(usize, &Turn)> {
        self.turns.iter().enumerate().find(|(_, t)| t.id.as_deref() == Some(uid))
    }
}

/// A bot's tool ceiling (opengrok-server `agui/ceiling.rs`, #268): the
/// rows its owner switched — `{name, kind, description, enabled,
/// available?, label?, connector?}` — and the `version` a PUT names so a
/// stale write is refused (409). No rows is the state before anyone
/// looked: every tool on. Once rows exist a name without one is off (a
/// ceiling only goes down, as `opengrok-policy` intersects it). Rows a
/// PUT sent for tools this broker does not know are kept, never dropped.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ceiling {
    pub rows: Vec<Value>,
    pub version: i64,
}

impl Ceiling {
    /// The row named `name`.
    pub fn row(&self, name: &str) -> Option<&Value> {
        self.rows.iter().find(|r| r.get("name").and_then(Value::as_str) == Some(name))
    }

    /// Whether the ceiling lets `name` through: its row's `enabled`; on,
    /// when nobody has set a ceiling yet; off otherwise.
    pub fn allows(&self, name: &str) -> bool {
        match self.row(name) {
            Some(r) => r.get("enabled").map(crate::js::truthy).unwrap_or(true),
            None => self.rows.is_empty(),
        }
    }

    /// The `tools` column: `{"rows": [...], "version": n}`; `None` for the
    /// untouched ceiling, which is what the migration leaves old rows at.
    pub fn to_json(&self) -> Option<Value> {
        if self.rows.is_empty() && self.version == 0 {
            return None;
        }
        Some(json!({ "rows": self.rows, "version": self.version }))
    }

    /// A ceiling from its column (or a bot JSON's `tools`); anything that
    /// is not that shape is the untouched ceiling.
    pub fn from_json(v: &Value) -> Self {
        let rows = v.get("rows").and_then(Value::as_array).cloned().unwrap_or_default();
        let version = v.get("version").and_then(Value::as_i64).unwrap_or(0);
        Self { rows, version }
    }
}

/// A bot (`store.bots[i]`).
#[derive(Clone, Debug, PartialEq)]
pub struct Bot {
    pub id: String,
    pub name: String,
    pub shape: String,
    pub color: String,
    /// Node's `threadId` (`null` once its chat is gone)
    pub thread_id: Option<String>,
    pub pinned: bool,
    pub section: String,
    pub hidden: bool,
    pub image: String,
    /// `null` when unset
    pub crop: Value,
    pub model: String,
    pub effort: String,
    pub fast: bool,
    pub label: String,
    /// `None` only for a bot written without one (it loads as the defaults)
    pub description: Option<String>,
    pub instructions: Option<String>,
    /// `[{id, text, at}]`
    pub memories: Vec<Value>,
    /// `[{id, kind, at, via}]`
    pub integrations: Vec<Value>,
    // made by another agent (agents.mjs): the bots table's `extra`
    pub made_by: Option<Value>,
    pub purpose: Option<Value>,
    pub helper: Option<Value>,
    pub accepts: Option<Value>,
    /// Bot Details' "Last updated": in memory only (never persisted)
    pub updated_at: Option<f64>,
    /// the bot's tool ceiling (app.db `bots.tools`)
    pub tools: Ceiling,
}

impl Bot {
    /// A new bot as `/bot/new` makes it.
    pub fn new(id: &str, name: &str, shape: &str, color: &str, thread_id: Option<String>) -> Self {
        let (description, instructions) = bot_defaults(name);
        Bot {
            id: id.into(),
            name: name.into(),
            shape: shape.into(),
            color: color.into(),
            thread_id,
            pinned: false,
            section: String::new(),
            hidden: false,
            image: String::new(),
            crop: Value::Null,
            model: String::new(),
            effort: String::new(),
            fast: false,
            label: String::new(),
            description: Some(description),
            instructions: Some(instructions),
            memories: Vec::new(),
            integrations: Vec::new(),
            made_by: None,
            purpose: None,
            helper: None,
            accepts: None,
            updated_at: None,
            tools: Ceiling::default(),
        }
    }
}

/// state.mjs `botDefaults`: (description, instructions).
pub fn bot_defaults(name: &str) -> (String, String) {
    (
        format!("{name} is a personal assistant bot in Buwiz Bot."),
        format!("Stay in character as {name}. Be helpful, concise and friendly; ask one short clarifying question when a request is ambiguous."),
    )
}

/// state.mjs `slugFile`: a module's display filename, kebab-case plus
/// ".tsx", or "" when nothing usable is left.
pub fn slug_file(s: &str) -> String {
    let s = s.trim_matches(crate::js::is_space);
    let s = match s.len().checked_sub(4).and_then(|k| s.get(k..).map(|tail| (k, tail))) {
        Some((k, tail)) if tail.eq_ignore_ascii_case(".tsx") => &s[..k],
        _ => s,
    };
    let lower = s.to_lowercase();
    let mut slug = String::new();
    let mut gap = false;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if gap {
                slug.push('-');
            }
            gap = false;
            slug.push(c);
        } else {
            gap = true;
        }
    }
    if gap {
        slug.push('-');
    }
    let slug = slug.trim_start_matches('-').trim_end_matches('-');
    let slug: String = slug.chars().take(40).collect();
    let slug = slug.trim_end_matches('-');
    if !slug.is_empty() && slug.len() <= 40 && slug.as_bytes()[0] != b'-' {
        format!("{slug}.tsx")
    } else {
        String::new()
    }
}

/// What the store last wrote (or read at load), to write only what changed.
#[derive(Clone, Debug, Default)]
pub(crate) struct Written {
    /// thread id → its row (title, hidden, archived, uid, routine, agents)
    pub threads: std::collections::HashMap<String, write::Row>,
    /// thread id → its ord
    pub ord: std::collections::HashMap<String, write::SqlValue>,
    /// thread id → its turns' rows, in seq order
    pub turns: std::collections::HashMap<String, Vec<write::Row>>,
    /// bot id → its row
    pub bots: std::collections::HashMap<String, write::Row>,
    /// meta active, sections, seq
    pub meta: std::collections::HashMap<&'static str, Option<String>>,
}

/// The store's memory (state.mjs `store`, `seq`, `threadSeq`).
#[derive(Clone, Debug, Default)]
pub struct State {
    /// the server's active chat (`store.active`)
    pub active: String,
    /// the `R<n>` counter (state.mjs `seq`)
    pub seq: u64,
    /// the next `t<n>` (state.mjs `threadSeq`)
    pub thread_seq: u64,
    /// the sidebar's sections (`store.sections`, a JSON array)
    pub sections: Value,
    pub list: Vec<Thread>,
    pub bots: Vec<Bot>,
    /// stand-in threads by their own key, never listed or written: an
    /// agent's request turn runs on one (agents-wire.mjs's `history`, the
    /// conversation from the recipient's side); [`State::thread`] finds
    /// them by that key
    pub stand_ins: std::collections::HashMap<String, Thread>,
    pub(crate) written: Written,
}

impl State {
    /// `findThread`: by `t<n>` id or uid.
    pub fn thread(&self, key: &str) -> Option<&Thread> {
        if key.is_empty() {
            return None;
        }
        if let Some(t) = self.stand_ins.get(key) {
            return Some(t);
        }
        self.list.iter().find(|t| t.id == key || t.uid.as_deref() == Some(key))
    }

    pub fn thread_mut(&mut self, key: &str) -> Option<&mut Thread> {
        if key.is_empty() {
            return None;
        }
        if self.stand_ins.contains_key(key) {
            return self.stand_ins.get_mut(key);
        }
        self.list.iter_mut().find(|t| t.id == key || t.uid.as_deref() == Some(key))
    }

    /// `activeThread()`: the active chat, else the first. The list is
    /// never empty once loaded.
    pub fn active_thread(&self) -> &Thread {
        let i = self.list.iter().position(|t| t.id == self.active).unwrap_or(0);
        &self.list[i]
    }

    pub fn active_index(&self) -> usize {
        self.list.iter().position(|t| t.id == self.active).unwrap_or(0)
    }

    /// `threadBot`: the bot that owns the thread, else (a routine's
    /// thread) the bot of the chat the routine reports to.
    pub fn thread_bot(&self, id: &str) -> Option<&Bot> {
        let t = self.thread(id);
        let own = |x: &str| self.bots.iter().find(|b| b.thread_id.as_deref() == Some(x));
        own(t.map(|t| t.id.as_str()).unwrap_or(id)).or_else(|| t.and_then(|t| t.routine.as_ref()).and_then(|r| r.get("thread")).and_then(Value::as_str).and_then(own))
    }

    pub fn bot(&self, id: &str) -> Option<&Bot> {
        self.bots.iter().find(|b| b.id == id)
    }

    pub fn bot_mut(&mut self, id: &str) -> Option<&mut Bot> {
        self.bots.iter_mut().find(|b| b.id == id)
    }

    /// `` `t${nextThreadSeq()}` ``.
    pub fn new_thread_id(&mut self) -> String {
        let n = self.thread_seq;
        self.thread_seq += 1;
        format!("t{n}")
    }

    /// `nextSeq()`: the next module number.
    pub fn next_module_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// A new chat `{id, uid, title, turns: []}` (not yet in the list).
    pub fn fresh_thread(&mut self, title: &str) -> Thread {
        Thread { id: self.new_thread_id(), uid: Some(uuidv7()), title: title.into(), ..Default::default() }
    }
}

/// The store: its memory, and the connection it writes through.
pub struct Store {
    data: PathBuf,
    db: Db,
    state: Mutex<State>,
    /// writeIndex's `lastModuleKey`
    index_key: Mutex<String>,
}

impl Store {
    /// An empty store over `data` (Node's `GROK_DATA_DIR`); [`Store::load`]
    /// reads it.
    pub fn new(data: PathBuf) -> Self {
        let db = Db::new(data.join("generated").join("app.db"));
        Self { data, db, state: Mutex::new(State::default()), index_key: Mutex::new(String::new()) }
    }

    pub fn data(&self) -> &Path {
        &self.data
    }

    pub fn gen_dir(&self) -> PathBuf {
        self.data.join("generated")
    }

    pub fn widgets_dir(&self) -> PathBuf {
        self.data.join("widgets")
    }

    /// The store's start (state.mjs's import and app-server.mjs's first
    /// steps): the directories Node makes, the schema, the rows (or the
    /// legacy JSON, or a first chat), the uid backfill, and the rows as
    /// `persist()` would write them (only those that differ).
    pub fn load(&self) -> Result<(), String> {
        for d in [self.gen_dir(), self.widgets_dir(), self.gen_dir().join("avatars"), self.gen_dir().join("attachments"), self.gen_dir().join("files")] {
            std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let gen = self.gen_dir();
        let mut s = self.db.with(|conn| Ok(schema::ensure(conn).and_then(|_| load::load(conn, &gen))))??;
        self.db.with(|conn| Ok(write::persist(conn, &mut s)))??;
        *self.lock() = s;
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn read<T>(&self, f: impl FnOnce(&State) -> T) -> T {
        f(&self.lock())
    }

    /// A change, then its rows written (see the module's docs).
    pub fn write<T>(&self, f: impl FnOnce(&mut State) -> T) -> Result<T, String> {
        let mut s = self.lock();
        let out = f(&mut s);
        self.db.with(|conn| Ok(write::persist(conn, &mut s)))??;
        Ok(out)
    }

    /// A change to what is never written.
    pub fn touch<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        f(&mut self.lock())
    }

    /// Pushes `turn` onto the thread (`t<n>` or uid), its id minted when
    /// it has none: where it landed, or `None` for no such thread.
    pub fn append_turn(&self, thread: &str, mut turn: Turn) -> Result<Option<TurnDone>, String> {
        let id = turn.id.get_or_insert_with(uuidv7).clone();
        self.write(|s| {
            let t = s.thread_mut(thread)?;
            t.turns.push(turn);
            Some(TurnDone { seq: t.turns.len() - 1, turn: id })
        })
    }

    /// Changes the turn whose id is `uid` in the thread; false when either
    /// is missing.
    pub fn update_turn(&self, thread: &str, uid: &str, f: impl FnOnce(&mut Turn)) -> Result<bool, String> {
        self.write(|s| match s.thread_mut(thread).and_then(|t| t.turns.iter_mut().find(|x| x.id.as_deref() == Some(uid))) {
            Some(turn) => {
                f(turn);
                true
            }
            None => false,
        })
    }

    /// Drops the turn whose id is `uid`; false when it isn't there.
    pub fn remove_turn(&self, thread: &str, uid: &str) -> Result<bool, String> {
        self.write(|s| {
            let Some(t) = s.thread_mut(thread) else { return false };
            let before = t.turns.len();
            t.turns.retain(|x| x.id.as_deref() != Some(uid));
            t.turns.len() != before
        })
    }

    /// A new chat titled `title`, first in the list (`unshift`) or last
    /// (`push`); `f` sets its marks. Answers its id and uid. The active
    /// chat is not changed.
    pub fn new_thread(&self, title: &str, first: bool, f: impl FnOnce(&mut Thread)) -> Result<(String, String), String> {
        self.write(|s| {
            let mut t = s.fresh_thread(title);
            f(&mut t);
            let ids = (t.id.clone(), t.uid.clone().unwrap_or_default());
            if first {
                s.list.insert(0, t);
            } else {
                s.list.push(t);
            }
            ids
        })
    }

    /// Sets a thread's `routine` and `agents` marks (`None` leaves one
    /// as it is, `Some(None)` clears it); false for no such thread.
    pub fn set_marks(&self, thread: &str, routine: Option<Option<Value>>, agents: Option<Option<Value>>) -> Result<bool, String> {
        self.write(|s| {
            let Some(t) = s.thread_mut(thread) else { return false };
            if let Some(r) = routine {
                t.routine = r;
            }
            if let Some(a) = agents {
                t.agents = a;
            }
            true
        })
    }

    /// `writeIndex()`'s file: generated/index.tsx, rewritten only when the
    /// set of modules, versions and published widgets changed.
    pub fn write_index(&self) {
        let mut key = self.index_key.lock().unwrap_or_else(|e| e.into_inner());
        index_tsx::write_index(&self.gen_dir(), &self.widgets_dir(), &mut key);
    }

    /// `writeShelf()`: widgets/index.tsx, the shelf template as is.
    pub fn write_shelf(&self) -> Result<(), String> {
        index_tsx::write_shelf(&self.widgets_dir())
    }
}
