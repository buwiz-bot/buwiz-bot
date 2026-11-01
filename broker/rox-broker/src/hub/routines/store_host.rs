//! [`RoutineHost`] on the hub's store (`ctx.store`, track A) and jobs
//! table (`ctx.jobs`): what routine-threads.mjs, routines.mjs and
//! state.mjs do to `store.list`, `store.bots` and `jobs`, through the
//! store's scoped writes (D33). Node's `writeIndex()` after a change is
//! `Store::write` (the rows) then `Store::write_index` (generated/index.tsx,
//! rewritten only when the modules changed).
//!
//! The store lives in the hub's context, which exists only once the hub is
//! built, so the adapter holds a weak handle filled by [`StoreHost::bind`]
//! from the hub's start (before the routines' own start). Before that, or
//! once the hub is gone, it answers as [`super::host::Unstored`] does.

use std::sync::{Arc, OnceLock, Weak};

use serde_json::{json, Value};

use super::host::{RoutineHost, Thread};
use crate::hub::ctx::HubCtx;
use crate::hub::jobs::JobId;
use crate::hub::store::{self, Turn};

#[derive(Default)]
pub struct StoreHost {
    ctx: OnceLock<Weak<HubCtx>>,
}

impl StoreHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// The hub's context, once it is up.
    pub fn bind(&self, ctx: &Arc<HubCtx>) {
        let _ = self.ctx.set(Arc::downgrade(ctx));
    }

    fn ctx(&self) -> Option<Arc<HubCtx>> {
        self.ctx.get()?.upgrade()
    }

    fn view(t: &store::Thread) -> Thread {
        Thread { id: t.id.clone(), uid: t.uid.clone().unwrap_or_default(), title: t.title.clone(), routine: t.routine.clone() }
    }

    /// `writeIndex()` after a change made through `Store::write`.
    fn saved<T>(ctx: &HubCtx, r: Result<T, String>) -> Option<T> {
        match r {
            Ok(v) => {
                ctx.store.write_index();
                Some(v)
            }
            Err(e) => {
                eprintln!("rox-broker: routines: the store's write: {e}");
                None
            }
        }
    }
}

/// Pending notices of `kind` whose `id` is `id` take `state`, in `chat`
/// or (`None`) every chat, as `settle_consent` settles consent cards; how
/// many changed.
pub fn settle_notices(ctx: &HubCtx, chat: Option<&str>, kind: &str, id: &str, state: &str) -> usize {
    let r = ctx.store.write(|s| {
        let mut changed = 0;
        let chats: Vec<String> = match chat {
            Some(c) => vec![c.to_owned()],
            None => s.list.iter().map(|t| t.id.clone()).collect(),
        };
        for c in chats {
            let Some(t) = s.thread_mut(&c) else { continue };
            for u in &mut t.turns {
                let Some(n) = u.notice.as_mut() else { continue };
                if n.get("kind") == Some(&json!(kind)) && n.get("id") == Some(&json!(id)) && n.get("state") == Some(&json!("pending")) {
                    n["state"] = json!(state);
                    changed += 1;
                }
            }
        }
        changed
    });
    match r {
        Ok(n) => {
            if n > 0 {
                ctx.store.write_index();
            }
            n
        }
        Err(e) => {
            eprintln!("rox-broker: notices: the store's write: {e}");
            0
        }
    }
}

impl RoutineHost for StoreHost {
    fn thread(&self, key: &str) -> Option<Thread> {
        self.ctx()?.store.read(|s| s.thread(key).map(Self::view))
    }

    fn routine_threads(&self) -> Vec<Thread> {
        let Some(ctx) = self.ctx() else { return Vec::new() };
        ctx.store.read(|s| s.list.iter().filter(|t| t.routine.is_some()).map(Self::view).collect())
    }

    fn add_routine_thread(&self, title: &str, mark: &Value) -> Thread {
        let fallback = || Thread { id: String::new(), uid: String::new(), title: title.to_owned(), routine: Some(mark.clone()) };
        let Some(ctx) = self.ctx() else { return fallback() };
        // `store.list.push({id: nextId(), uid: uid(), title, turns: [], routine: mark})`
        let r = ctx.store.write(|s| {
            let mut t = s.fresh_thread(title);
            t.routine = Some(mark.clone());
            let view = Self::view(&t);
            s.list.push(t);
            view
        });
        Self::saved(&ctx, r).unwrap_or_else(fallback)
    }

    fn set_routine_mark(&self, id: &str, mark: &Value, title: Option<&str>) {
        let Some(ctx) = self.ctx() else { return };
        let r = ctx.store.write(|s| {
            if let Some(t) = s.thread_mut(id) {
                t.routine = Some(mark.clone());
                if let Some(title) = title {
                    t.title = title.to_owned();
                }
            }
        });
        Self::saved(&ctx, r);
    }

    fn push_notice(&self, chat: &str, text: &str, notice: &Value) -> bool {
        let Some(ctx) = self.ctx() else { return false };
        match ctx.store.append_turn(chat, Turn::notice(text, notice.clone())) {
            Ok(Some(_)) => {
                ctx.store.write_index();
                true
            }
            Ok(None) => false,
            // the turn stays in memory (Node's `save()` throwing after the
            // push), and the next write retries it
            Err(e) => {
                eprintln!("rox-broker: routines: the store's write: {e}");
                true
            }
        }
    }

    fn settle_consent(&self, chat: &str, instance: &str, state: &str) {
        let Some(ctx) = self.ctx() else { return };
        let r = ctx.store.write(|s| {
            let Some(t) = s.thread_mut(chat) else { return false };
            let mut changed = false;
            for u in &mut t.turns {
                let Some(n) = u.notice.as_mut() else { continue };
                if n.get("kind") == Some(&json!("routine_consent")) && n.get("instance") == Some(&json!(instance)) && n.get("state") == Some(&json!("pending")) {
                    // `{...u.notice, state}`: the key keeps its place
                    n["state"] = json!(state);
                    changed = true;
                }
            }
            changed
        });
        if let Ok(true) = r {
            ctx.store.write_index();
        } else if let Err(e) = r {
            eprintln!("rox-broker: routines: the store's write: {e}");
        }
    }

    fn bot_name(&self, thread: &str) -> Option<String> {
        self.ctx()?.store.read(|s| s.bots.iter().find(|b| b.thread_id.as_deref() == Some(thread)).map(|b| b.name.clone()))
    }

    fn bot_chat(&self, thread: &str) -> Option<String> {
        self.ctx()?.store.read(|s| s.thread_bot(thread).and_then(|b| b.thread_id.clone()))
    }

    fn linked_account(&self, is_email: &dyn Fn(&str) -> bool) -> Option<String> {
        self.ctx()?.store.read(|s| {
            for t in &s.list {
                for u in t.turns.iter().rev() {
                    for l in u.links.as_ref().and_then(Value::as_array).into_iter().flatten() {
                        if let Some(a) = l.get("account").and_then(Value::as_str).filter(|a| is_email(a)) {
                            return Some(a.to_owned());
                        }
                    }
                }
            }
            None
        })
    }

    /// Node holds the job object itself; a job the table no longer has
    /// (only `/chat`'s cap of 25 drops one) counts as done.
    fn job_done(&self, job: JobId) -> bool {
        self.ctx().and_then(|c| c.jobs.with(job, |j| j.done)).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::store::{Bot, Store};
    use crate::hub::traits::Clock;

    /// A loaded store on a temp dir, the adapter bound to it.
    fn bound(label: &str) -> (Arc<HubCtx>, StoreHost) {
        let d = std::env::temp_dir().join(format!("rox-store-host-{label}-{}", store::uuidv7()));
        let ctx = HubCtx::new(d, Clock::manual());
        ctx.store.load().unwrap();
        let ctx = Arc::new(ctx);
        let host = StoreHost::new();
        assert!(host.thread("t1").is_none(), "unbound: no chat");
        host.bind(&ctx);
        (ctx, host)
    }

    /// What app.db holds, read by a fresh store.
    fn reloaded(ctx: &HubCtx) -> Store {
        let s = Store::new(ctx.data.clone());
        s.load().unwrap();
        s
    }

    #[test]
    fn store_host_threads_notices_and_consent_reach_app_db() {
        let (ctx, host) = bound("threads");
        let first = host.thread("t1").expect("the first chat");
        assert_eq!(host.thread(&first.uid).map(|t| t.id), Some("t1".to_owned()), "by uid");
        assert!(host.thread("").is_none());
        let mark = json!({"plugin": "p", "id": "r", "instance": "ri_1", "name": "R", "thread": "t1"});
        let home = host.add_routine_thread("R", &mark);
        assert_eq!((home.id.as_str(), home.title.as_str(), home.routine.as_ref()), ("t2", "R", Some(&mark)));
        assert_eq!(host.routine_threads().iter().map(|t| t.id.clone()).collect::<Vec<_>>(), ["t2"]);
        let renamed = json!({"plugin": "p", "id": "r", "instance": "ri_1", "name": "R · q", "thread": "t1"});
        host.set_routine_mark("t2", &renamed, Some("R · q"));
        let consent = json!({"kind": "routine_consent", "instance": "ri_1", "state": "pending", "name": "R"});
        assert!(host.push_notice(&first.uid, "Enable R here?", &consent));
        assert!(host.push_notice("t1", "R ran · 1 new item", &json!({"kind": "routine", "instance": "ri_1"})));
        assert!(!host.push_notice("t9", "x", &json!({})), "no such chat");
        host.settle_consent("t1", "ri_2", "enabled");
        host.settle_consent("t1", "ri_1", "enabled");

        let s = reloaded(&ctx);
        s.read(|s| {
            let t2 = s.thread("t2").unwrap();
            assert_eq!((t2.title.as_str(), t2.routine.as_ref(), t2.uid.as_deref()), ("R · q", Some(&renamed), Some(home.uid.as_str())));
            let t1 = s.thread("t1").unwrap();
            assert_eq!(t1.turns.iter().map(|u| (u.role.as_str(), u.text.as_deref().unwrap())).collect::<Vec<_>>(), [("notice", "Enable R here?"), ("notice", "R ran · 1 new item")]);
            assert_eq!(t1.turns[0].notice, Some(json!({"kind": "routine_consent", "instance": "ri_1", "state": "enabled", "name": "R"})), "settled, the key in its place");
            assert!(t1.turns.iter().all(|u| u.id.is_some()));
        });
    }

    #[test]
    fn store_host_bots_linked_accounts_and_jobs() {
        let (ctx, host) = bound("bots");
        ctx.store
            .write(|s| {
                s.bots.push(Bot::new("ann", "Ann", "star", "#FF309B", Some("t1".to_owned())));
                s.list[0].turns.push(Turn { links: Some(json!([{"account": "nope"}, {"account": "a@b.co"}])), ..Turn::user("hi") });
            })
            .unwrap();
        let home = host.add_routine_thread("R", &json!({"plugin": "p", "id": "r", "instance": "ri_1", "name": "R", "thread": "t1"}));
        assert_eq!(host.bot_name("t1").as_deref(), Some("Ann"));
        assert_eq!(host.bot_name(&home.id), None, "only the bot's own chat");
        assert_eq!(host.bot_chat(&home.id).as_deref(), Some("t1"), "a routine's thread answers as its chat's bot");
        assert_eq!(host.linked_account(&|a| a.contains('@')).as_deref(), Some("a@b.co"));
        let job = ctx.jobs.create("t1", Value::Null, None, false);
        assert!(!host.job_done(job));
        ctx.jobs.finish(&ctx.store, job, "replied", Some("reply"));
        assert!(host.job_done(job));
        assert!(host.job_done(999), "a job the table no longer has");
    }
}
