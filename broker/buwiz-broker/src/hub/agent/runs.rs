//! A run from start to end, however it ends. [`under_way`] wraps the two entry points of
//! [`Agent`](super::Agent): while it runs, the job can be stopped and has a row in the `runs`
//! table. A stop drops the turn's future, and with it what it holds: the card it parked (`Parked`
//! expires it), its hand-over (`Handing` ends it), any command it was waiting on. Its screen's
//! `Hold` lives in the `Run`, which the entry point drops as it returns.
//!
//! The row goes when the run ends, and only then: a broker shutting down drops its tasks, which
//! isn't an end, and one that's killed drops nothing. A panic is an end: it unwinds through
//! [`Marks`], which takes the row. So a row found at the next start ([`cut_off`]) is a run a
//! restart ended, and the chat is told.
//!
//! Each run also raises `job_mark`, kept when its row goes, so the next start's job ids go on
//! after every run the last broker had, not only the ones it left a row for.

use std::future::Future;
use std::sync::Arc;

use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::hub::ctx::HubCtx;
use crate::hub::jobs::JobId;
use crate::hub::tools::miniapps::post_in;

/// A stopped run's job detail, and its notice's reason.
pub const STOPPED: &str = "stopped";
/// A notice's reason: a restart ended the run.
pub const RESTART: &str = "restart";

/// What a `run_ended` notice says. The model never reads it: notices stay out of its history.
pub fn said(reason: &str) -> &'static str {
    if reason == RESTART {
        "The Bot's service restarted, so this run ended before it finished."
    } else {
        "Stopped before it finished."
    }
}

/// The `run_ended` notice.
pub fn notice(reason: &str) -> Value {
    json!({"kind": "run_ended", "reason": reason})
}

/// What a task that's gone can't clear up itself.
struct Marks<'a> {
    ctx: &'a HubCtx,
    job: JobId,
}

impl Drop for Marks<'_> {
    fn drop(&mut self) {
        // a panic ends a task with its job unfinished, and an app would follow that for ever
        if std::thread::panicking() {
            self.ctx.jobs.finish(&self.ctx.store, self.job, "failed — the run crashed", None);
            forget(self.ctx, self.job);
        }
        self.ctx.jobs.with(self.job, |j| j.abort = None);
    }
}

/// The run has ended: its row goes.
fn forget(ctx: &HubCtx, job: JobId) {
    if let Err(e) = ctx.db.with(|c| c.execute("DELETE FROM runs WHERE job=?", [job as i64])) {
        eprintln!("buwiz-broker: runs: {e}");
    }
}

/// Runs `turn` as job `job` on `thread`: `None` when a person stopped it, its future dropped.
pub async fn under_way<T>(ctx: &HubCtx, job: JobId, thread: &str, turn: impl Future<Output = T>) -> Option<T> {
    let at = ctx.clock.now() as i64;
    let row = ctx.db.with(|c| {
        c.execute("INSERT INTO job_mark(id, high) VALUES(1, ?) ON CONFLICT(id) DO UPDATE SET high=max(high, excluded.high)", [job as i64])?;
        c.execute("INSERT OR REPLACE INTO runs(job, thread, at) VALUES(?,?,?)", params![job as i64, thread, at])
    });
    if let Err(e) = row {
        eprintln!("buwiz-broker: runs: {e}");
    }
    let _marks = Marks { ctx, job };
    let abort = Arc::new(Notify::new());
    ctx.jobs.with(job, |j| j.abort = Some(abort.clone()));
    let ended = tokio::select! {
        biased;
        () = abort.notified() => None,
        done = turn => Some(done),
    };
    forget(ctx, job);
    ended
}

/// On start: each run the last broker left behind gets its notice, and job ids go on after them.
pub fn cut_off(ctx: &HubCtx) {
    match ctx.db.with(|c| c.query_row("SELECT high FROM job_mark", [], |r| r.get::<_, i64>(0)).optional()) {
        Ok(Some(high)) => ctx.jobs.start_after(high as JobId),
        Ok(None) => {}
        Err(e) => eprintln!("buwiz-broker: runs: {e}"),
    }
    let rows = ctx.db.with(|c| {
        let mut st = c.prepare("SELECT job, thread FROM runs ORDER BY job")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    });
    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => return eprintln!("buwiz-broker: runs: {e}"),
    };
    for (job, thread) in rows {
        // a chat deleted since has nobody to tell
        post_in(ctx, &thread, &notice(RESTART));
        ctx.jobs.start_after(job as JobId);
    }
    if let Err(e) = ctx.db.with(|c| c.execute("DELETE FROM runs", [])) {
        eprintln!("buwiz-broker: runs: {e}");
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use bytes::Bytes;
    use http_body_util::BodyExt;
    use hyper::{HeaderMap, Method};
    use tokio::sync::mpsc;

    use super::*;
    use crate::hub::agent::Agent;
    use crate::hub::model::Settings;
    use crate::hub::routes::chat::{job_stop, jobs_running};
    use crate::hub::routes::host;
    use crate::hub::store::Bot;
    use crate::hub::traits::{ChatTurn, Clock, ModelApi, TurnRequest, TurnRunner};
    use crate::hub::types::{BoxFuture, HubRequest, HubResponse, TurnCtx};

    /// A model that answers its script, then never answers again, and counts what it's asked.
    #[derive(Default)]
    pub(crate) struct Script {
        pub answers: Mutex<VecDeque<Value>>,
        pub calls: AtomicUsize,
        /// each request's body
        pub bodies: Mutex<Vec<Value>>,
    }

    impl Script {
        pub(crate) fn tool(&self, name: &str, input: Value) {
            let call = json!([{"type": "tool_use", "id": format!("tu{}", self.answers.lock().unwrap().len()), "name": name, "input": input}]);
            self.answers.lock().unwrap().push_back(json!({"id": "msg", "type": "message", "role": "assistant", "model": "m", "content": call, "stop_reason": "tool_use", "usage": {"input_tokens": 1, "output_tokens": 1}}));
        }

        pub(crate) fn say(&self, text: &str) {
            self.answers.lock().unwrap().push_back(json!({"id": "msg", "type": "message", "role": "assistant", "model": "m", "content": [{"type": "text", "text": text}], "stop_reason": "end_turn", "usage": {"input_tokens": 1, "output_tokens": 1}}));
        }

        fn next(&self, body: Value) -> Option<Value> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.bodies.lock().unwrap().push(body);
            self.answers.lock().unwrap().pop_front()
        }
    }

    impl ModelApi for Script {
        fn create(&self, _: &TurnCtx, body: Value) -> BoxFuture<'static, Result<Value, String>> {
            let m = self.next(body);
            Box::pin(async move {
                match m {
                    Some(m) => Ok(m),
                    None => std::future::pending().await,
                }
            })
        }

        fn stream(&self, _: &TurnCtx, body: Value) -> BoxFuture<'static, Result<mpsc::Receiver<Result<Value, String>>, String>> {
            let m = self.next(body);
            Box::pin(async move {
                let Some(m) = m else { return std::future::pending().await };
                let (tx, rx) = mpsc::channel(8);
                let mut start = m.clone();
                start["stop_reason"] = Value::Null;
                for e in [json!({"type": "message_start", "message": start}), json!({"type": "message_delta", "delta": {"stop_reason": m["stop_reason"]}, "usage": {"output_tokens": 1}}), json!({"type": "message_stop"})] {
                    let _ = tx.send(Ok(e)).await;
                }
                Ok(rx)
            })
        }
    }

    pub(crate) fn agent() -> Agent {
        Agent::new(Arc::new(Settings::from(|_| None)))
    }

    /// `/chat`'s job on `thread`, running in a task.
    pub(crate) fn chat(ctx: &Arc<HubCtx>, thread: &str) -> (JobId, tokio::task::JoinHandle<Result<(), String>>) {
        let job = ctx.jobs.create(thread, Value::Null, None, true);
        let turn = ChatTurn { turn: TurnCtx { thread: thread.to_owned(), job: Some(job), ..TurnCtx::default() }, prompt: "go".to_owned(), extra: json!({}), retry: None };
        (job, tokio::spawn(agent().generate(ctx.clone(), turn)))
    }

    /// What a person's Stop sends.
    pub(crate) async fn stop(ctx: &Arc<HubCtx>, job: JobId) -> HubResponse {
        let mut headers = HeaderMap::new();
        headers.insert("x-buwiz-host", host::TEST_TOKEN.parse().unwrap());
        job_stop(ctx.clone(), HubRequest { method: Method::POST, url: "/job/stop".to_owned(), headers, body: Bytes::from(json!({"id": job}).to_string()), params: Vec::new() }).await
    }

    pub(crate) async fn until(what: &str, check: impl Fn() -> bool) {
        for _ in 0..1000 {
            if check() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("{what}");
    }

    pub(crate) fn rows(ctx: &HubCtx) -> i64 {
        ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))).unwrap()
    }

    pub(crate) fn ended(ctx: &HubCtx, thread: &str) -> Vec<(String, String)> {
        ctx.store.read(|s| {
            s.thread(thread).unwrap().turns.iter().filter(|u| u.notice.as_ref().is_some_and(|n| n["kind"] == "run_ended")).map(|u| (u.notice.as_ref().unwrap()["reason"].as_str().unwrap().to_owned(), u.text.clone().unwrap())).collect()
        })
    }

    fn temp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("buwiz-runs-{label}-{}-{}", std::process::id(), crate::hub::store::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A store on `dir` with the model `script`; → it and a Bot's chat.
    fn hub_on(dir: &Path, script: Option<Arc<Script>>) -> (Arc<HubCtx>, String) {
        let mut ctx = HubCtx::new(dir.to_owned(), Clock::manual());
        ctx.store.load().unwrap();
        ctx.local = Some(Arc::new(crate::hub::model::testkit::Local::default()));
        ctx.model = script.map(|s| s as Arc<dyn ModelApi>);
        let thread = ctx.store.read(|s| s.bots.first().and_then(|b| b.thread_id.clone()));
        let thread = thread.unwrap_or_else(|| {
            ctx.store
                .write(|s| {
                    let t = s.fresh_thread("Ann");
                    let id = t.id.clone();
                    s.list.push(t);
                    s.bots.push(Bot::new("b_ann", "Ann", "blob", "#FFFFFF", Some(id.clone())));
                    id
                })
                .unwrap()
        });
        (Arc::new(ctx), thread)
    }

    /// A broker that's shut down drops its tasks mid-run: that's a restart, not an end. (One
    /// that's killed drops nothing, so it leaves the same row.)
    #[test]
    fn a_run_cut_by_a_restart_leaves_a_notice() {
        let dir = temp("cut");
        let (first, thread) = hub_on(&dir, Some(Arc::new(Script::default())));
        first.jobs.start_after(40);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let job = runtime.block_on(async {
            let (job, _hung) = chat(&first, &thread);
            until("the run never started", || rows(&first) == 1).await;
            job
        });
        drop(runtime);

        let (next, _) = hub_on(&dir, None);
        assert_eq!(rows(&next), 1, "the row is on disk, where the next broker reads it");
        cut_off(&next);
        assert_eq!(ended(&next, &thread), [(RESTART.to_owned(), said(RESTART).to_owned())]);
        assert_eq!(rows(&next), 0, "told once");
        cut_off(&next);
        assert_eq!(ended(&next, &thread).len(), 1, "a second start says nothing");

        // an app still following the old job reads that it's gone, and never a new job's answer
        assert_eq!(next.jobs.json(job as f64)["phase"], "unknown");
        assert!(next.jobs.create(&thread, Value::Null, None, true) > job);
    }

    /// A crash loses the jobs table, and a run that ended has no row to say how far ids got.
    #[tokio::test]
    async fn job_ids_go_on_after_a_run_that_left_no_row() {
        let dir = temp("mark");
        let script = Arc::new(Script::default());
        script.answers.lock().unwrap().push_back(json!({"id": "m", "type": "message", "role": "assistant", "model": "m", "content": [{"type": "text", "text": "Done."}], "stop_reason": "end_turn", "usage": {"input_tokens": 1, "output_tokens": 1}}));
        let (first, thread) = hub_on(&dir, Some(script));
        first.jobs.start_after(40);
        let (job, done) = chat(&first, &thread);
        done.await.unwrap().unwrap();
        assert_eq!(rows(&first), 0);

        let (next, _) = hub_on(&dir, None);
        cut_off(&next);
        assert_eq!(next.jobs.json(job as f64)["phase"], "unknown", "an app following the old job never reads a new one");
        assert!(next.jobs.create(&thread, Value::Null, None, true) > job);

        // a lower job can't lower the mark
        let (later, _) = hub_on(&dir, None);
        under_way(&later, 3, &thread, async {}).await;
        let (last, _) = hub_on(&dir, None);
        cut_off(&last);
        assert!(last.jobs.create(&thread, Value::Null, None, true) > job);
    }

    /// A panic ended the run: no restart did, whatever the next start finds.
    #[tokio::test]
    async fn a_run_that_panicked_is_not_called_cut_off_by_a_restart() {
        let dir = temp("panic");
        let (ctx, thread) = hub_on(&dir, None);
        let job = ctx.jobs.create(&thread, Value::Null, None, true);
        let (c, t) = (ctx.clone(), thread.clone());
        let done = tokio::spawn(async move { under_way(&c, job, &t, async { panic!("a turn's bug") }).await });
        assert!(done.await.unwrap_err().is_panic());
        assert_eq!(rows(&ctx), 0, "a panic ended it");
        assert_eq!(ctx.jobs.json(job as f64)["detail"], "failed — the run crashed");
        cut_off(&ctx);
        assert!(ended(&ctx, &thread).is_empty());
    }

    #[tokio::test]
    async fn a_run_that_ended_leaves_no_notice_at_the_next_start() {
        let dir = temp("clean");
        let script = Arc::new(Script::default());
        script.answers.lock().unwrap().push_back(json!({"id": "m", "type": "message", "role": "assistant", "model": "m", "content": [{"type": "text", "text": "Done."}], "stop_reason": "end_turn", "usage": {"input_tokens": 1, "output_tokens": 1}}));
        let (ctx, thread) = hub_on(&dir, Some(script));
        let (_, done) = chat(&ctx, &thread);
        done.await.unwrap().unwrap();
        assert_eq!(rows(&ctx), 0);
        cut_off(&ctx);
        assert!(ended(&ctx, &thread).is_empty());
    }

    #[tokio::test]
    async fn a_run_under_way_is_listed_for_an_app_that_wasnt_there_and_gone_when_stopped() {
        let (ctx, thread) = hub_on(&temp("list"), Some(Arc::new(Script::default())));
        let (job, done) = chat(&ctx, &thread);
        until("the run never started", || !ctx.jobs.running().is_empty()).await;
        let listed = |ctx: Arc<HubCtx>| async move {
            let res = jobs_running(ctx, HubRequest { method: Method::GET, url: "/jobs/running".to_owned(), headers: HeaderMap::new(), body: Bytes::new(), params: Vec::new() }).await;
            serde_json::from_slice::<Value>(&res.into_body().collect().await.unwrap().to_bytes()).unwrap()
        };
        let v = listed(ctx.clone()).await;
        assert_eq!((&v["jobs"][0]["id"], &v["jobs"][0]["thread"], &v["jobs"][0]["bot"]), (&json!(job), &json!(thread), &json!("b_ann")), "{v}");
        assert_eq!(stop(&ctx, job).await.status(), 200);
        done.await.unwrap().unwrap();
        assert_eq!(listed(ctx.clone()).await["jobs"], json!([]));
        assert_eq!(stop(&ctx, job).await.status(), 404, "there's nothing left to stop");
        assert_eq!(ended(&ctx, &thread), [(STOPPED.to_owned(), said(STOPPED).to_owned())]);
    }

    #[tokio::test]
    async fn a_stop_ends_a_turn_nobody_typed_and_a_routine_doesnt_read_it_as_a_failure() {
        let (ctx, thread) = hub_on(&temp("background"), Some(Arc::new(Script::default())));
        let turn = TurnRequest { turn: TurnCtx { thread: thread.clone(), routine: Some("r1".to_owned()), ..TurnCtx::default() }, shown: "Routine · Mail".to_owned(), prompt: "go".to_owned(), mark: None };
        let done = tokio::spawn(agent().run(ctx.clone(), turn));
        until("the run never started", || !ctx.jobs.running().is_empty()).await;
        let job = ctx.jobs.running()[0].0;
        assert_eq!(stop(&ctx, job).await.status(), 200);
        let done = done.await.unwrap().expect("a stop isn't an error");
        assert_eq!(ctx.store.read(|s| s.thread(&thread).unwrap().turns[done.seq].text.clone()).as_deref(), Some("Routine · Mail"));
        assert_eq!(ctx.jobs.json(job as f64)["detail"], STOPPED);
        assert_eq!(rows(&ctx), 0);
    }

    #[test]
    fn a_long_run_outlives_the_cap_on_finished_jobs() {
        let ctx = HubCtx::new(temp("cap"), Clock::manual());
        let long = ctx.jobs.create("t1", Value::Null, None, true);
        for _ in 0..30 {
            let id = ctx.jobs.create("t1", Value::Null, None, true);
            ctx.jobs.with(id, |j| j.done = true);
        }
        assert_eq!(ctx.jobs.json(long as f64)["phase"], "queued");
    }

    fn sources(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                sources(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs" || x == "json") {
                out.push((p.clone(), std::fs::read_to_string(&p).unwrap()));
            }
        }
    }

    /// What can stop a run is the route, and the route answers only the host token: the model's
    /// reach ends at its tools, and a Bot's shell can reach the port but not the token.
    #[test]
    fn no_model_tool_can_stop_a_run() {
        let post = Method::POST;
        assert!(host::locked_out(&post, "/job/stop", &HeaderMap::new()), "the front door lets it through without the token");
        let mut token = HeaderMap::new();
        token.insert("x-buwiz-host", host::TEST_TOKEN.parse().unwrap());
        assert!(!host::locked_out(&post, "/job/stop", &token));

        let get = Method::GET;
        assert!(host::locked_out(&get, "/jobs/running", &HeaderMap::new()), "a Bot's shell would read every run");
        assert!(!host::locked_out(&get, "/jobs/running", &token));
        assert!(!host::locked_out(&get, "/job/stop", &HeaderMap::new()) && !host::locked_out(&get, "/skills", &HeaderMap::new()), "a locked POST's GET stays as it was");

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut all = Vec::new();
        sources(&root, &mut all);
        assert!(all.len() > 100);
        let (call, route) = (concat!(".stop", "("), concat!("\"/job", "/stop\""));
        let this = Path::new(file!()).file_name().unwrap();
        let mut callers = Vec::new();
        for (path, text) in all.iter().filter(|(p, _)| p.file_name() != Some(this)) {
            let rel = path.strip_prefix(&root).unwrap().display().to_string();
            if text.contains(&format!("jobs{call}")) {
                callers.push(rel.clone());
            }
            if text.contains(route) {
                assert!(["hub/routes.rs", "hub/routes/chat.rs", "hub/routes/host.rs"].contains(&rel.as_str()), "{rel} names the route");
            }
        }
        assert_eq!(callers, ["hub/routes/chat.rs"], "only the route stops a run");
    }
}
