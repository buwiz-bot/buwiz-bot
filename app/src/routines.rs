//! Routines, managed with the bot they belong to: the Details pane of a
//! chat lists the routines that report to it (`pane.rs`), each with Run
//! now, Pause or Resume, and the way to its thread; the server's own (v0)
//! routines also Delete. Below them, Suggested lists the templates
//! connected servers announce (`GET /routines/templates`); Enable makes
//! one in the open chat (`POST /routine/from-template`).
//!
//! Run now: a plugin routine's (`POST /v1/host/routines/run`) answers at
//! once and runs in the broker; Rox shows "Running…" on its row until the
//! plugin list says it is done, then the run's result line for a moment.
//! A v0 routine's (`POST /routine/run`) answers once its tick is done.
//! The keys, states and lines are here; the calls in `mini_v1/ui.rs`.

use nsh_base::input;
use nsh_core::prelude::*;

use crate::live::store;
use crate::mini_v1::connect;
use crate::theme;

/// One routine the server runs (`GET /routines`): every `every_s`
/// seconds it checks a tool (`check`, cheap) and wakes the agent in its
/// chat only for what's new — or runs the agent every time without one.
#[derive(Clone, PartialEq, Default)]
pub struct Routine {
    pub id: String,
    pub name: String,
    pub thread: String,
    pub thread_title: String,
    pub bot_name: String,
    pub every_s: f64,
    pub enabled: bool,
    /// The tool it checks: `server › tool`, or "".
    pub check: String,
    /// Epoch ms of the last run (0 never).
    pub last_run: f64,
    pub last_error: String,
}

impl Routine {
    pub(crate) fn of(v: &serde_json::Value) -> Self {
        let s = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_owned();
        let n = |k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
        let check = v.get("check").filter(|c| c.is_object()).map(|c| format!("{} › {}", s(c, "server"), s(c, "tool")));
        // a time in seconds reads as ms
        let last = n("last_run");
        Routine {
            id: v.get("id").map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).unwrap_or_default(),
            name: s(v, "name"),
            thread: s(v, "thread"),
            thread_title: s(v, "threadTitle"),
            bot_name: s(v, "botName"),
            every_s: n("every_s"),
            enabled: v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false),
            check: check.unwrap_or_default(),
            last_run: if last > 0.0 && last < 1e11 { last * 1000.0 } else { last },
            last_error: s(v, "last_error"),
        }
    }
}

/// "every minute", "every 5 min", "every 2 h"
pub(crate) fn interval(every_s: f64) -> String {
    let s = every_s.max(0.0).round() as i64;
    match s {
        0..=60 => "every minute".to_owned(),
        _ if s % 3600 == 0 => format!("every {} h", s / 3600),
        _ if s < 3600 => format!("every {} min", (s + 30) / 60),
        _ => format!("every {:.1} h", s as f64 / 3600.0),
    }
}

/// A routine a connected server suggests (`GET /routines/templates`).
/// One that can't run yet says why (`reason`).
#[derive(Clone, PartialEq, Default)]
pub struct RoutineTemplate {
    pub server: String,
    pub id: String,
    pub name: String,
    pub description: String,
    pub every_s: f64,
    pub available: bool,
    pub reason: String,
    /// Enable asks for the user's Google address first.
    pub needs_email: bool,
}

impl RoutineTemplate {
    fn of(v: &serde_json::Value) -> Self {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_owned();
        let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        RoutineTemplate {
            server: s("server"),
            id: s("id"),
            name: s("name"),
            description: s("description"),
            every_s: v.get("every_s").and_then(|x| x.as_f64()).unwrap_or(0.0),
            available: b("available"),
            reason: s("reason"),
            needs_email: b("needs_email"),
        }
    }
}

/// `GET /routines` into the Details pane.
pub(crate) fn load_routines() {
    Owner::detached().run(|| {
        spawn(async move {
            if let Some(v) = crate::client::get("/routines").await {
                let list = v
                    .get("routines")
                    .and_then(|x| x.as_array())
                    .map(|l| l.iter().map(Routine::of).collect())
                    .unwrap_or_default();
                store().routines.set(list);
            }
        })
    });
}

/// `GET /routines/templates` into Suggested — on its own, as it may wait
/// on servers starting.
pub(crate) fn load_templates() {
    Owner::detached().run(|| {
        spawn(async move {
            if let Some(v) = crate::client::get("/routines/templates").await {
                let list = v.as_array().map(|l| l.iter().map(RoutineTemplate::of).collect()).unwrap_or_default();
                store().routine_templates.set(list);
            }
        })
    });
}


// ——————————————————————————— Run now ———————————————————————————

/// A routine's Run now, as its row shows it.
#[derive(Debug, Clone, PartialEq)]
pub enum RunState {
    /// Asked for and not done yet: "Running…".
    Running,
    /// Done or refused: the result line, and whether it is a failure.
    Done { line: String, failed: bool },
}

/// The key of a plugin routine's run.
pub(crate) fn v1_key(plugin: &str, id: &str) -> String {
    format!("v1:{plugin}/{id}")
}

/// The key of a v0 routine's run.
pub(crate) fn v0_key(id: &str) -> String {
    format!("v0:{id}")
}

/// A finished plugin routine run, from the routine as the broker lists it
/// after: its error, or how many new items it ran.
pub(crate) fn v1_outcome(r: &connect::Routine) -> (String, bool) {
    if !r.last_error.is_empty() {
        return (format!("Run failed: {}", r.last_error), true);
    }
    match r.last_items {
        Some(0) | None => ("Ran just now · nothing new".to_owned(), false),
        Some(1) => ("Ran just now · 1 new item".to_owned(), false),
        Some(n) => (format!("Ran just now · {n} new items"), false),
    }
}

/// A finished v0 routine run, from the routine the server answered with.
pub(crate) fn v0_outcome(r: &Routine) -> (String, bool) {
    if r.last_error.is_empty() {
        ("Ran just now".to_owned(), false)
    } else {
        (format!("Run failed: {}", r.last_error), true)
    }
}

/// A Run now the server refused ("… is paused; resume it first").
pub(crate) fn refused(message: &str) -> (String, bool) {
    if message.is_empty() {
        ("Couldn't run it".to_owned(), true)
    } else {
        (format!("Couldn't run: {message}"), true)
    }
}

/// The status line a row shows: its run's state over its usual line.
pub(crate) fn row_status(run: Option<&RunState>, usual: (String, bool)) -> (String, bool) {
    match run {
        Some(RunState::Running) => ("Running…".to_owned(), false),
        Some(RunState::Done { line, failed }) => (line.clone(), *failed),
        None => usual,
    }
}

/// The run state of `key`, tracked.
pub(crate) fn run_of(key: &str) -> Option<RunState> {
    store().routine_runs.with(|l| l.iter().find(|(k, _)| k == key).map(|(_, s)| s.clone()))
}

/// Whether `key` is running now (untracked).
pub(crate) fn is_running(key: &str) -> bool {
    store().routine_runs.with_untracked(|l| l.iter().any(|(k, s)| k == key && *s == RunState::Running))
}

/// Sets `key`'s state.
pub(crate) fn set_run(key: &str, state: RunState) {
    store().routine_runs.update(|l| {
        l.retain(|(k, _)| k != key);
        l.push((key.to_owned(), state));
    });
}

/// A run is done: its line shows for a while, then the row's own again.
pub(crate) fn finish(key: String, (line, failed): (String, bool)) {
    let done = RunState::Done { line, failed };
    set_run(&key, done.clone());
    Owner::detached().run(|| {
        spawn(async move {
            crate::vault::off_main(|| std::thread::sleep(std::time::Duration::from_secs(20))).await;
            store().routine_runs.update(|l| l.retain(|(k, s)| !(*k == key && *s == done)));
        })
    });
}

/// Run now of a v0 routine (`POST /routine/run {id}`): its turn posts in
/// its chat; the answer comes once the tick is done.
pub(crate) fn run_v0(id: String) {
    let key = v0_key(&id);
    if is_running(&key) {
        return;
    }
    set_run(&key, RunState::Running);
    Owner::detached().run(|| {
        spawn(async move {
            let v = crate::client::post_any("/routine/run", serde_json::json!({ "id": id })).await;
            let line = match v {
                Some(v) if v.get("ok").and_then(|x| x.as_bool()) == Some(true) => {
                    v0_outcome(&Routine::of(v.get("routine").unwrap_or(&serde_json::Value::Null)))
                }
                Some(v) => refused(v.get("error").and_then(|x| x.as_str()).unwrap_or_default()),
                None => refused("the app server didn't answer"),
            };
            finish(key, line);
            load_routines();
        })
    });
}

/// The params editor's text as params: a JSON object, else why not.
/// The broker checks it against the routine's schema.
pub(crate) fn parse_params(text: &str) -> Result<serde_json::Value, String> {
    match serde_json::from_str::<serde_json::Value>(text.trim()) {
        Ok(v) if v.is_object() => Ok(v),
        Ok(_) => Err("Params are a JSON object: {\"name\": value}".to_owned()),
        Err(e) => Err(format!("Not JSON: {e}")),
    }
}

/// On / off (`POST /routine/update {id, enabled}`), shown at once.
pub(crate) fn set_routine_enabled(id: String, enabled: bool) {
    store().routines.update(|l| {
        for r in l.iter_mut().filter(|r| r.id == id) {
            r.enabled = enabled;
        }
    });
    Owner::detached().run(|| {
        spawn(async move {
            let v = crate::client::post("/routine/update", serde_json::json!({ "id": id, "enabled": enabled })).await;
            if !v.as_ref().is_some_and(|v| v.get("ok").and_then(|x| x.as_bool()) == Some(true)) {
                crate::toast::show("Couldn't change the routine".to_owned());
            }
            load_routines();
        })
    });
}

/// Delete (`POST /routine/delete {id}`).
pub(crate) fn delete_routine(id: String) {
    store().routines.update(|l| l.retain(|r| r.id != id));
    Owner::detached().run(|| {
        spawn(async move {
            let v = crate::client::post("/routine/delete", serde_json::json!({ "id": id })).await;
            if !v.as_ref().is_some_and(|v| v.get("ok").and_then(|x| x.as_bool()) == Some(true)) {
                crate::toast::show("Couldn't delete the routine".to_owned());
            }
            load_routines();
        })
    });
}

// ——————————————————————————— Suggested ———————————————————————————

/// Enable on a template: makes it in the open chat (a bot's routine runs
/// in its chat). One that needs the Google address asks for it in its
/// card first.
fn enable_here(t: &RoutineTemplate) {
    let st = store();
    let email = st.routine_email.get_untracked().trim().to_owned();
    if t.needs_email && email.is_empty() {
        st.routine_ask_email.set(Some((t.server.clone(), t.id.clone())));
        return;
    }
    let thread = st.active_id.get_untracked();
    if thread.is_empty() {
        return crate::toast::show("Open the chat the routine should post in");
    }
    let place = st.active_bot().map(|b| b.name).unwrap_or_else(|| "this chat".to_owned());
    enable_template(t.server.clone(), t.id.clone(), thread, place, email);
}

/// The chat picked for a template (`POST /routine/from-template`); then
/// it shows in the chat's routines. The server keeps the Google address
/// for the next one.
pub(crate) fn enable_template(server: String, id: String, thread: String, place: String, email: String) {
    Owner::detached().run(|| {
        spawn(async move {
            let mut body = serde_json::json!({ "server": server, "id": id, "thread": thread });
            if !email.is_empty() {
                body["user_google_email"] = email.into();
            }
            let v = crate::client::post_any("/routine/from-template", body).await;
            let st = store();
            let s = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_owned();
            match v {
                Some(v) if v.get("ok").and_then(|x| x.as_bool()) == Some(true) => {
                    let name = v.get("routine").map(|r| s(r, "name")).unwrap_or_default();
                    crate::toast::show(format!("{name} runs in {place}"));
                    st.routine_ask_email.set(None);
                    st.routine_email.set(String::new());
                }
                Some(v) => {
                    if s(&v, "need") == "user_google_email" {
                        st.routine_ask_email.set(Some((server, id)));
                    }
                    let error = s(&v, "error");
                    crate::toast::show(if error.is_empty() { "Couldn't enable the routine".to_owned() } else { error });
                }
                None => crate::toast::show("The app server didn't answer".to_owned()),
            }
            load_routines();
            load_templates();
        })
    });
}

/// Suggested: the servers' templates, none when there are none.
pub(crate) fn suggested() -> impl IntoElement {
    dynamic(|| {
        let list = store().routine_templates.get();
        if list.is_empty() {
            return view().into_element();
        }
        view()
            .pt(px(14.0))
            .gap(px(8.0))
            .child(text("Suggested").text_size(px(13.0)).text_color(theme::DIM).px_3())
            .children(list.into_iter().map(template_row))
            .into_element()
    })
}

/// A template: its name and Enable; its server and interval; what it
/// does; why it can't run yet; the Google address when Enable asks.
fn template_row(t: RoutineTemplate) -> impl IntoElement {
    let (server, id) = (t.server.clone(), t.id.clone());
    let asking = move || store().routine_ask_email.get().is_some_and(|(s, i)| s == server && i == id);
    let (t1, t2, t3) = (t.clone(), t.clone(), t.clone());
    let hov = signal(false);
    view()
        .rounded(px(10.0))
        .border_1()
        .border_color(theme::BORDER_S)
        .px(px(12.0))
        .py(px(10.0))
        .gap(px(4.0))
        .label(t.name.clone())
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    text(t.name.clone())
                        .flex_1()
                        .min_w(px(0.0))
                        .number_of_lines(1)
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::TEXT),
                )
                .when(t.available, |v| {
                    v.child(
                        text("Enable")
                            .text_size(px(12.0))
                            .text_color(move || if hov.get() { theme::TEXT } else { theme::MUTED })
                            .px_2()
                            .py_1()
                            .rounded(px(6.0))
                            .bg(theme::HOVER)
                            .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
                            .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
                            .on_press(move || enable_here(&t1))
                            .label("Enable routine"),
                    )
                }),
        )
        .child(
            text(format!("{} · {}", t.server, interval(t.every_s)))
                .number_of_lines(1)
                .text_size(px(12.0))
                .text_color(theme::MUTED),
        )
        .when(!t.description.is_empty(), |v| {
            v.child(text(t.description.clone()).number_of_lines(3).text_size(px(12.0)).text_color(theme::MUTED))
        })
        .when(!t.available && !t.reason.is_empty(), |v| {
            v.child(text(t.reason.clone()).number_of_lines(3).text_size(px(12.0)).text_color(theme::DOWN))
        })
        .child(dynamic(move || {
            if !asking() {
                return view().into_element();
            }
            let (t2, t3) = (t2.clone(), t3.clone());
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .pt_1()
                .child(
                    input()
                        .auto_focus(true)
                        .value(move || store().routine_email.get())
                        .placeholder("Your Google address, for the Gmail search")
                        .placeholder_color(|| color(theme::GHOST))
                        .text_size(px(13.0))
                        .text_color(theme::TEXT)
                        .on_change(|v: &str| store().routine_email.set(v.to_owned()))
                        .on_submit(move |_: &str| enable_here(&t2))
                        .flex_1()
                        .h(px(30.0))
                        .px_2()
                        .rounded(px(7.0))
                        .bg(theme::HOVER),
                )
                .child(
                    text("Continue")
                        .text_size(px(12.0))
                        .text_color(theme::TEXT)
                        .px_2()
                        .py(px(6.0))
                        .rounded(px(7.0))
                        .bg(theme::HOVER)
                        .on_press(move || enable_here(&t3)),
                )
                .into_element()
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_shows_running_then_its_result_then_the_row_line() {
        let usual = ("Last ran 5 min ago".to_owned(), false);
        assert_eq!(row_status(None, usual.clone()), usual);
        assert_eq!(row_status(Some(&RunState::Running), usual.clone()), ("Running…".to_owned(), false));
        let done = RunState::Done { line: "Ran just now · 2 new items".into(), failed: false };
        assert_eq!(row_status(Some(&done), usual), ("Ran just now · 2 new items".to_owned(), false));
    }

    #[test]
    fn a_plugin_run_says_what_it_found_or_why_it_failed() {
        let mut r = connect::Routine { enabled: true, available: true, last_run: 1.0, last_items: Some(0), ..Default::default() };
        assert_eq!(v1_outcome(&r), ("Ran just now · nothing new".to_owned(), false));
        r.last_items = Some(1);
        assert_eq!(v1_outcome(&r).0, "Ran just now · 1 new item");
        r.last_items = Some(3);
        assert_eq!(v1_outcome(&r).0, "Ran just now · 3 new items");
        r.last_error = "fx_query: offline".into();
        assert_eq!(v1_outcome(&r), ("Run failed: fx_query: offline".to_owned(), true));
        assert_eq!(refused("BIR payments is paused; resume it first"), ("Couldn't run: BIR payments is paused; resume it first".to_owned(), true));
        assert_eq!(refused(""), ("Couldn't run it".to_owned(), true));
    }

    #[test]
    fn params_are_a_json_object() {
        assert_eq!(parse_params(r#" {"taxpayers": ["tp_1"]} "#), Ok(serde_json::json!({"taxpayers": ["tp_1"]})));
        assert!(parse_params("[1]").unwrap_err().starts_with("Params are a JSON object"));
        assert!(parse_params("{taxpayers").unwrap_err().starts_with("Not JSON"));
    }

    #[test]
    fn a_v0_run_reads_the_routine_the_server_answers_with() {
        let ok = Routine::of(&serde_json::json!({"id": "r1", "name": "Mail", "last_run": 1793194720000u64}));
        assert_eq!(v0_outcome(&ok), ("Ran just now".to_owned(), false));
        let bad = Routine::of(&serde_json::json!({"id": "r1", "last_error": "google-workspace isn't connected"}));
        assert_eq!(v0_outcome(&bad), ("Run failed: google-workspace isn't connected".to_owned(), true));
        assert_ne!(v1_key("p", "a"), v0_key("p/a"));
    }
}
