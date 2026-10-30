//! The jobs table (state.mjs `jobs`, `setJob`, `jobJson`): what `/status`
//! reads while a chat turn runs. A job keeps its last 30 events, so a
//! poller sees a status that lasted less than its interval; `/chat` keeps
//! the last 25 jobs (`/retry` and other starters don't trim).
//!
//! The tool loop (5.4) works on a job through [`Jobs::set`] (`setJob`),
//! [`Jobs::finish`] (`setJob(job, "done", …)`, `done`, `result`) and
//! [`Jobs::with`] for the rest of its fields ([`Job::turn`], the building
//! placeholder `setJob` keeps its status on; [`Job::revising`];
//! [`Job::open`], what `/status` shows as `open`; [`Job::fields`] for
//! its own scratch: `pendingApp`, `cards`, `_phrase`, …).

use std::collections::VecDeque;
use std::sync::Mutex;

use serde_json::{json, Value};

use super::store::Store;

/// A job's id (Node's `nextJobId`, counting from 1).
pub type JobId = u64;

#[derive(Clone, Debug, PartialEq)]
pub struct JobEvent {
    pub seq: u64,
    pub phase: String,
    pub detail: String,
}

#[derive(Clone, Debug, Default)]
pub struct Job {
    pub id: JobId,
    /// the thread's `t<n>` id
    pub thread: String,
    pub seq: u64,
    pub phase: String,
    pub detail: String,
    pub done: bool,
    pub result: Option<String>,
    pub events: VecDeque<JobEvent>,
    /// `/chat`'s `job.extra` (`reply`, `attachments`, `v1Attachments`)
    pub extra: Value,
    /// `/retry`'s card name
    pub retry: Option<String>,
    /// a mini app a server asked to show (`{key, title}`), `/status`'s `open`
    pub open: Option<Value>,
    /// the building placeholder's turn id (`job.turn`)
    pub turn: Option<String>,
    /// the card a revise covers (`job.revising`): its turn id
    pub revising: Option<String>,
    /// the tool loop's own fields
    pub fields: serde_json::Map<String, Value>,
}

#[derive(Default)]
struct Table {
    next: JobId,
    /// insertion order, as a Map iterates
    jobs: Vec<Job>,
}

#[derive(Default)]
pub struct Jobs {
    t: Mutex<Table>,
}

impl Jobs {
    fn lock(&self) -> std::sync::MutexGuard<'_, Table> {
        self.t.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A queued job on `thread` (`allocJobId()` and `jobs.set`); `cap`
    /// keeps the newest 25 (`/chat`'s `if (jobs.size > 25) …`).
    pub fn create(&self, thread: &str, extra: Value, retry: Option<String>, cap: bool) -> JobId {
        let mut t = self.lock();
        t.next = t.next.max(1);
        let id = t.next;
        t.next += 1;
        t.jobs.push(Job { id, thread: thread.into(), phase: "queued".into(), extra, retry, ..Default::default() });
        if cap && t.jobs.len() > 25 {
            t.jobs.remove(0);
        }
        id
    }

    /// The job's fields, changed or read.
    pub fn with<T>(&self, id: JobId, f: impl FnOnce(&mut Job) -> T) -> Option<T> {
        self.lock().jobs.iter_mut().find(|j| j.id == id).map(f)
    }

    /// `setJob(job, phase, detail)`: the next event; a building card shows
    /// it as its status; "done" clears a revise mark and drops a card
    /// still building.
    pub fn set(&self, store: &Store, id: JobId, phase: &str, detail: &str) {
        let Some((thread, turn, revising)) = self.with(id, |job| {
            job.seq += 1;
            job.phase = phase.into();
            job.detail = detail.into();
            job.events.push_back(JobEvent { seq: job.seq, phase: phase.into(), detail: detail.into() });
            if job.events.len() > 30 {
                job.events.pop_front();
            }
            let revising = if phase == "done" { job.revising.take() } else { None };
            (job.thread.clone(), job.turn.clone(), revising)
        }) else {
            return;
        };
        let status = if detail.is_empty() { phase } else { detail };
        let drop = store.touch(|s| {
            if let Some(uid) = &revising {
                for t in s.list.iter_mut() {
                    for u in t.turns.iter_mut().filter(|u| u.id.as_deref() == Some(uid)) {
                        u.revising = false;
                    }
                }
            }
            let Some(uid) = &turn else { return false };
            let Some(t) = s.list.iter_mut().find(|t| t.id == thread) else { return false };
            let Some(i) = t.turns.iter().position(|u| u.id.as_deref() == Some(uid) && u.building) else { return false };
            t.turns[i].status = Some(status.into());
            if phase == "done" {
                t.turns.remove(i);
                return true;
            }
            false
        });
        if drop {
            self.with(id, |job| job.turn = None);
        }
    }

    /// The job's end: `setJob(job, "done", detail)`, then `done` and its
    /// `result` (a crash leaves `result` null).
    pub fn finish(&self, store: &Store, id: JobId, detail: &str, result: Option<&str>) {
        self.set(store, id, "done", detail);
        self.with(id, |job| {
            job.done = true;
            if let Some(r) = result {
                job.result = Some(r.into());
            }
        });
    }

    /// `jobJson(job)`, or the unknown job's answer for `id` (`/status`).
    pub fn json(&self, id: f64) -> Value {
        let t = self.lock();
        let job = (id.fract() == 0.0 && id >= 1.0).then(|| t.jobs.iter().find(|j| j.id as f64 == id)).flatten();
        match job {
            Some(j) => json!({
                "id": j.id,
                "seq": j.seq,
                "phase": j.phase,
                "detail": j.detail,
                "done": j.done,
                "result": j.result,
                "events": j.events.iter().map(|e| json!({"seq": e.seq, "phase": e.phase, "detail": e.detail})).collect::<Vec<_>>(),
                "open": j.open.clone().unwrap_or(Value::Null),
            }),
            None => json!({
                "id": if id.is_finite() { serde_json::Number::from_f64(id).map(Value::Number).unwrap_or(Value::Null) } else { Value::Null },
                "seq": 0,
                "phase": "unknown",
                "detail": "",
                "done": true,
                "events": [],
                "open": Value::Null,
            }),
        }
    }

    /// Whether any job on `thread` is still running (agents' idle wait).
    pub fn busy(&self, thread: &str) -> bool {
        self.lock().jobs.iter().any(|j| j.thread == thread && !j.done)
    }
}
