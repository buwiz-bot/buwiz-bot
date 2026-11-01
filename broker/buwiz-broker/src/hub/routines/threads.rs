//! Where a v1 routine's runs land (`v1/routine-threads.mjs`, spec 5.9):
//! each instance has its own thread, made when it is enabled and reused by
//! every run, titled by the routine's name (and the instance's params) and
//! marked `routine: {plugin, id, instance, name, thread}`, `thread` being
//! the chat the instance reports to. That chat gets one `notice` turn per
//! run that ran items or failed, and the consent cards. The thread work
//! goes through the store seam ([`RoutineHost`]).

use serde_json::{json, Value};

use super::host::{RoutineHost, Thread};
use super::text::approved_text;
use crate::js;

/// The notice's one line: "<name> ran · 2 new items" / "<name> failed: …";
/// a consent card's question (`noticeText`); a mini app's install,
/// uninstall or sign-in (`account_required`) card's.
pub fn notice_text(n: &Value) -> String {
    let s = |k: &str| n.get(k).map(js::string_of).unwrap_or_default();
    let list = |k: &str| n.get(k).and_then(Value::as_array).filter(|l| !l.is_empty()).map(|l| l.iter().map(js::string_of).collect::<Vec<_>>());
    match n.get("kind").and_then(Value::as_str) {
        Some("app_install") => return format!("Install {} {} from {}?", s("title"), s("version"), s("source")),
        Some("app_uninstall") => return format!("Uninstall {}? It removes its pins, attachments and data.", s("title")),
        Some("account_required") => {
            let to = if s("provider") == "google" { "Google".to_owned() } else { s("label") };
            return format!("Sign in to {to} for {}", s("title"));
        }
        _ => {}
    }
    if n.get("kind").and_then(Value::as_str) == Some("routine_consent") {
        let summary = n.get("summary").filter(|v| js::truthy(v));
        let what = match summary {
            Some(sum) => format!("{} ({})", s("name"), js::string_of(sum)),
            None => s("name"),
        };
        let may = list("writes").map(|w| format!(" It may change {} data without asking: {}.", s("title"), w.join(", "))).unwrap_or_default();
        let reads = list("reads").map(|r| format!(" It reads {}.", r.join("; "))).unwrap_or_default();
        return format!("Enable {what} here?{may}{reads}");
    }
    match n.get("error").filter(|e| js::truthy(e)) {
        Some(e) => format!("{} failed: {}", s("name"), approved_text(&js::string_of(e), 200)),
        None => {
            let items = n.get("items").and_then(Value::as_f64).unwrap_or(f64::NAN);
            format!("{} ran · {} new item{}", s("name"), js::string_of(n.get("items").unwrap_or(&Value::Null)), if items == 1.0 { "" } else { "s" })
        }
    }
}

fn mark_of(t: &Thread) -> Option<&Value> {
    t.routine.as_ref()
}

fn is(v: Option<&Value>, s: &str) -> bool {
    v.and_then(Value::as_str) == Some(s)
}

/// The instance's thread, if it has one (`find`); `None` is Node's
/// `undefined` instance (a mark without one).
pub fn find(host: &dyn RoutineHost, plugin: &str, routine: &str, instance: Option<&str>) -> Option<Thread> {
    host.routine_threads().into_iter().find(|t| mark_of(t).is_some_and(|m| is(m.get("plugin"), plugin) && is(m.get("id"), routine) && m.get("instance").and_then(Value::as_str) == instance))
}

/// Gives a routine's thread from before instances to `instance` (`adopt`).
pub fn adopt(host: &dyn RoutineHost, plugin: &str, routine: &str, instance: &str) {
    let found = host.routine_threads().into_iter().find(|t| mark_of(t).is_some_and(|m| is(m.get("plugin"), plugin) && is(m.get("id"), routine) && !m.get("instance").is_some_and(js::truthy)));
    if let Some(t) = found {
        let mut mark = t.routine.clone().unwrap_or_else(|| json!({}));
        mark["instance"] = json!(instance);
        host.set_routine_mark(&t.id, &mark, None);
    }
}

/// The instance's thread, made the first time it is asked for, renamed
/// or moved when its name or chat changed (`ensure`).
pub fn ensure(host: &dyn RoutineHost, plugin: &str, routine: &str, instance: Option<&str>, name: &str, chat: &str) -> Thread {
    let mut mark = json!({"plugin": plugin, "id": routine});
    if let Some(i) = instance {
        mark["instance"] = json!(i);
    }
    mark["name"] = json!(name);
    mark["thread"] = json!(chat);
    match find(host, plugin, routine, instance) {
        None => host.add_routine_thread(name, &mark),
        Some(t) => {
            let m = t.routine.clone().unwrap_or(Value::Null);
            if !is(m.get("name"), name) || !is(m.get("thread"), chat) {
                host.set_routine_mark(&t.id, &mark, Some(name));
                Thread { title: name.to_owned(), routine: Some(mark), ..t }
            } else {
                t
            }
        }
    }
}

/// Appends a run's notice (or a consent card) to the chat `chat`; `false`
/// when it is gone (`notice`).
pub fn notice(host: &dyn RoutineHost, chat: &str, n: &Value) -> bool {
    host.push_notice(chat, &notice_text(n), n)
}

#[cfg(test)]
mod tests {
    use super::super::host::{MemBot, MemoryHost};
    use super::*;

    /// routine-threads.test.mjs: a routine's thread, its item turns and the
    /// chat's notice (the store keeps them; here, their shape).
    #[test]
    fn routine_threads_a_routines_thread_its_item_turns_and_the_chats_notice() {
        let host = MemoryHost::new(0);
        host.add_chat("t1", "uid-chat", "first chat");
        host.lock().bots.push(MemBot { name: "Piccolo".into(), thread: "t1".into() });
        let h = ensure(&host, "dev.example.bir", "payments", None, "BIR payments", "t1");
        let again = ensure(&host, "dev.example.bir", "payments", None, "BIR payments", "t1");
        assert_eq!(again.id, h.id, "ensure reuses the thread");
        assert_eq!(h.title, "BIR payments");
        assert_eq!(h.routine, Some(json!({"plugin": "dev.example.bir", "id": "payments", "name": "BIR payments", "thread": "t1"})));
        assert_eq!(host.bot_chat(&h.id).as_deref(), Some("t1"), "it answers as the reporting chat's bot");
        let n = json!({"kind": "routine", "plugin": "dev.example.bir", "routine": "payments", "name": "BIR payments", "thread": h.id, "thread_uid": h.uid, "run": "run_1", "seq": 0, "turn": "x", "items": 1});
        assert!(notice(&host, "t1", &n));
        let last = host.turns("t1").pop().unwrap();
        assert_eq!(last["role"], "notice");
        assert_eq!(last["text"], "BIR payments ran · 1 new item");
        assert_eq!(last["notice"]["thread"], json!(h.id));
        assert!(!notice(&host, "t404", &n), "a deleted chat takes no notice");
        // renamed in an update: the same thread, its title and mark follow
        let renamed = ensure(&host, "dev.example.bir", "payments", None, "BIR payments · q: x", "t1");
        assert_eq!((renamed.id.as_str(), renamed.title.as_str()), (h.id.as_str(), "BIR payments · q: x"));
    }

    #[test]
    fn notice_texts_for_runs_failures_and_consent() {
        assert_eq!(notice_text(&json!({"name": "A", "items": 2})), "A ran · 2 new items");
        assert_eq!(notice_text(&json!({"name": "A", "items": 0, "error": "boom"})), "A failed: boom");
        let card = json!({"kind": "routine_consent", "name": "Fixture writer", "title": "Fixture", "summary": "From the agent", "writes": ["fx_echo"], "reads": []});
        assert_eq!(notice_text(&card), "Enable Fixture writer (From the agent) here? It may change Fixture data without asking: fx_echo.");
        let reads = json!({"kind": "routine_consent", "name": "W", "title": "F", "summary": "", "writes": [], "reads": ["G: a, b", "H: c"]});
        assert_eq!(notice_text(&reads), "Enable W here? It reads G: a, b; H: c.");
        let install = json!({"kind": "app_install", "id": "app.buwiz.x", "title": "X", "version": "1.1.0", "source": "https://github.com/o/r/releases/tag/v1.1.0", "state": "pending"});
        assert_eq!(notice_text(&install), "Install X 1.1.0 from https://github.com/o/r/releases/tag/v1.1.0?");
        let uninstall = json!({"kind": "app_uninstall", "id": "app.buwiz.x", "title": "X", "version": "1.1.0", "state": "pending"});
        assert_eq!(notice_text(&uninstall), "Uninstall X? It removes its pins, attachments and data.");
    }
}
