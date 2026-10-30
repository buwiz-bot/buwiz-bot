//! What routines need from the store (track A's `hub/store`), as one
//! narrow trait: Node's routines read and change `store.list` (threads and
//! their turns) and `store.bots` through `state.mjs`, and save with
//! `writeIndex()`. Each method says which Node code it stands for. The
//! main session implements it on the store; [`MemoryHost`] is the store's
//! shape in memory (the JS tests hand `routine-threads.mjs` a plain
//! object), and [`Unstored`] stands in until the adapter lands: no chat
//! exists, so nothing that needs one runs.

use std::sync::Mutex;

use serde_json::{json, Value};

use crate::hub::jobs::JobId;

/// A thread as routines see it.
#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    /// `t<n>`
    pub id: String,
    pub uid: String,
    pub title: String,
    /// the `routine` mark: `{plugin, id, instance, name, thread}` on a v1
    /// routine's own thread
    pub routine: Option<Value>,
}

pub trait RoutineHost: Send + Sync {
    /// `findThread(key)` (state.mjs): the thread whose id or uid is `key`
    /// (none for "").
    fn thread(&self, key: &str) -> Option<Thread>;

    /// The threads that carry a `routine` mark, in `store.list` order
    /// (routine-threads.mjs `find` and `adopt` search them).
    fn routine_threads(&self) -> Vec<Thread>;

    /// routine-threads.mjs `ensure`, the new thread: `{id: "t" +
    /// nextThreadSeq(), uid: uuidv7(), title, turns: [], routine: mark}`
    /// pushed onto `store.list`, then `writeIndex()`.
    fn add_routine_thread(&self, title: &str, mark: &Value) -> Thread;

    /// `t.routine = mark` and, when given, `t.title = title`, then
    /// `writeIndex()` (routine-threads.mjs `ensure` renaming, `adopt`).
    fn set_routine_mark(&self, id: &str, mark: &Value, title: Option<&str>);

    /// routine-threads.mjs `notice`: `{id: uuidv7(), role: "notice", text,
    /// notice}` pushed onto the turns of the thread whose id or uid is
    /// `chat`, then `writeIndex()`; `false` when there is no such thread.
    fn push_notice(&self, chat: &str, text: &str, notice: &Value) -> bool;

    /// routine-threads.mjs `settle`: in the thread whose id or uid is
    /// `chat`, every turn whose `notice` has kind `routine_consent`, this
    /// `instance` and state `pending` gets `notice = {...notice, state}`
    /// (the key keeps its place); `writeIndex()` when one changed.
    fn settle_consent(&self, chat: &str, instance: &str, state: &str);

    /// `store.bots.find((b) => b.threadId === thread)?.name` (routines.mjs
    /// `routineJson`'s `botName`).
    fn bot_name(&self, thread: &str) -> Option<String>;

    /// `threadBot(id)?.threadId` (state.mjs): the chat of the bot the
    /// thread answers as: its own bot's, else, for a routine's thread, the
    /// bot of the chat it reports to (routines.mjs `chatsOf`).
    fn bot_chat(&self, thread: &str) -> Option<String>;

    /// routines.mjs `googleEmail`'s `linked()`: over `store.list` in order,
    /// each thread's turns newest first, each turn's `links[].account`:
    /// the first one `is_email` accepts.
    fn linked_account(&self, is_email: &dyn Fn(&str) -> bool) -> Option<String>;

    /// Whether the job `job` is done (`job.done` in state.mjs's `jobs`):
    /// routines.mjs `afterJob` waits for it.
    fn job_done(&self, job: JobId) -> bool;
}

/// Until the store adapter lands: no thread, no bot, every job done.
pub struct Unstored;

impl RoutineHost for Unstored {
    fn thread(&self, _: &str) -> Option<Thread> {
        None
    }
    fn routine_threads(&self) -> Vec<Thread> {
        Vec::new()
    }
    fn add_routine_thread(&self, title: &str, mark: &Value) -> Thread {
        Thread { id: String::new(), uid: String::new(), title: title.to_owned(), routine: Some(mark.clone()) }
    }
    fn set_routine_mark(&self, _: &str, _: &Value, _: Option<&str>) {}
    fn push_notice(&self, _: &str, _: &str, _: &Value) -> bool {
        false
    }
    fn settle_consent(&self, _: &str, _: &str, _: &str) {}
    fn bot_name(&self, _: &str) -> Option<String> {
        None
    }
    fn bot_chat(&self, _: &str) -> Option<String> {
        None
    }
    fn linked_account(&self, _: &dyn Fn(&str) -> bool) -> Option<String> {
        None
    }
    fn job_done(&self, _: JobId) -> bool {
        true
    }
}

/// One thread of [`MemoryHost`]: its turns as JSON, Node's shape.
#[derive(Clone, Debug)]
pub struct MemThread {
    pub id: String,
    pub uid: String,
    pub title: String,
    pub routine: Option<Value>,
    pub turns: Vec<Value>,
}

/// A bot of [`MemoryHost`]: its name and the chat that is its own.
#[derive(Clone, Debug)]
pub struct MemBot {
    pub name: String,
    pub thread: String,
}

#[derive(Default)]
pub struct MemState {
    pub list: Vec<MemThread>,
    pub bots: Vec<MemBot>,
    pub done: Vec<JobId>,
    /// the next thread number and uid counter
    pub next: u64,
}

/// The store's shape in memory, as the JS tests build it.
#[derive(Default)]
pub struct MemoryHost {
    pub state: Mutex<MemState>,
}

impl MemoryHost {
    pub fn new(next: u64) -> Self {
        Self { state: Mutex::new(MemState { next, ..MemState::default() }) }
    }

    /// Adds a chat (`{id, uid, title, turns: []}`).
    pub fn add_chat(&self, id: &str, uid: &str, title: &str) {
        self.lock().list.push(MemThread { id: id.to_owned(), uid: uid.to_owned(), title: title.to_owned(), routine: None, turns: Vec::new() });
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, MemState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The thread `id`'s turns.
    pub fn turns(&self, id: &str) -> Vec<Value> {
        self.lock().list.iter().find(|t| t.id == id).map(|t| t.turns.clone()).unwrap_or_default()
    }

    /// Drops the thread `id` (a deleted chat).
    pub fn remove(&self, id: &str) {
        self.lock().list.retain(|t| t.id != id);
    }

    fn view(t: &MemThread) -> Thread {
        Thread { id: t.id.clone(), uid: t.uid.clone(), title: t.title.clone(), routine: t.routine.clone() }
    }
}

impl RoutineHost for MemoryHost {
    fn thread(&self, key: &str) -> Option<Thread> {
        if key.is_empty() {
            return None;
        }
        self.lock().list.iter().find(|t| t.id == key || t.uid == key).map(Self::view)
    }

    fn routine_threads(&self) -> Vec<Thread> {
        self.lock().list.iter().filter(|t| t.routine.is_some()).map(Self::view).collect()
    }

    fn add_routine_thread(&self, title: &str, mark: &Value) -> Thread {
        let mut s = self.lock();
        s.next += 1;
        let id = format!("t{}", 100 + s.next);
        s.next += 1;
        let uid = format!("uid-{}", s.next);
        let t = MemThread { id, uid, title: title.to_owned(), routine: Some(mark.clone()), turns: Vec::new() };
        s.list.push(t.clone());
        Self::view(&t)
    }

    fn set_routine_mark(&self, id: &str, mark: &Value, title: Option<&str>) {
        if let Some(t) = self.lock().list.iter_mut().find(|t| t.id == id) {
            t.routine = Some(mark.clone());
            if let Some(title) = title {
                t.title = title.to_owned();
            }
        }
    }

    fn push_notice(&self, chat: &str, text: &str, notice: &Value) -> bool {
        let mut s = self.lock();
        s.next += 1;
        let id = format!("uid-{}", s.next);
        match s.list.iter_mut().find(|t| !chat.is_empty() && (t.id == chat || t.uid == chat)) {
            Some(t) => {
                t.turns.push(json!({"id": id, "role": "notice", "text": text, "notice": notice}));
                true
            }
            None => false,
        }
    }

    fn settle_consent(&self, chat: &str, instance: &str, state: &str) {
        let mut s = self.lock();
        let Some(t) = s.list.iter_mut().find(|t| !chat.is_empty() && (t.id == chat || t.uid == chat)) else { return };
        for u in &mut t.turns {
            let Some(n) = u.get_mut("notice") else { continue };
            if n.get("kind") == Some(&json!("routine_consent")) && n.get("instance") == Some(&json!(instance)) && n.get("state") == Some(&json!("pending")) {
                n["state"] = json!(state);
            }
        }
    }

    fn bot_name(&self, thread: &str) -> Option<String> {
        self.lock().bots.iter().find(|b| b.thread == thread).map(|b| b.name.clone())
    }

    fn bot_chat(&self, thread: &str) -> Option<String> {
        let s = self.lock();
        let t = s.list.iter().find(|t| !thread.is_empty() && (t.id == thread || t.uid == thread));
        let own = |x: &str| s.bots.iter().find(|b| b.thread == x).map(|b| b.thread.clone());
        own(t.map_or(thread, |t| t.id.as_str())).or_else(|| t.and_then(|t| t.routine.as_ref()?.get("thread")?.as_str().map(str::to_owned)).and_then(|c| own(&c)))
    }

    fn linked_account(&self, is_email: &dyn Fn(&str) -> bool) -> Option<String> {
        let s = self.lock();
        for t in &s.list {
            for u in t.turns.iter().rev() {
                for l in u.get("links").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(a) = l.get("account").and_then(Value::as_str).filter(|a| is_email(a)) {
                        return Some(a.to_owned());
                    }
                }
            }
        }
        None
    }

    fn job_done(&self, job: JobId) -> bool {
        self.lock().done.contains(&job)
    }
}
