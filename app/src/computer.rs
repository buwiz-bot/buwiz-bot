//! The Computer tab, and a computer full screen.
//!
//! A computer is shared; a screen is this Bot's. The tab shows the computer's state and this
//! Bot's screen as a tile that changes when the screen does. Clicking a sleeping computer wakes
//! it, for every Bot at it. Opening shows the live desktop in a web view inside the Bot, on the
//! full-screen stack under the key `computer:<bot>`: the same bar, Pin and Close as a mini app.
//! voxl's page needs WebCodecs, which `WKWebView` has on `127.0.0.1`, a secure context.
//!
//! The app holds no secret: the broker's `/open` answers with a one-time ticket on its viewer
//! gate, spent by the time anything could read the URL. A picture in the tab is only the screen.
//! `ui_look` never reads this: only mini app views register with the Hub.

use nsh_core::prelude::*;
use serde_json::{json, Value};

use crate::client;
use crate::live::{keep, store};
use crate::store::spawn_detached;
use crate::theme::{self, tc};

const KEY_PREFIX: &str = "computer:";
const POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// What the broker says about this Bot's computer.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Status {
    /// `absent`, `asleep`, `waking`, `awake` or `error`
    pub state: String,
    pub message: String,
    /// the Bots at this computer, this one included: (id, name)
    pub bots: Vec<(String, String)>,
    pub wake_ms: Option<u64>,
}

impl Status {
    pub fn parse(v: &Value) -> Self {
        let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        let bots = v.get("bots").and_then(Value::as_array).into_iter().flatten();
        Self {
            state: text("state"),
            message: text("message"),
            bots: bots.filter_map(|b| Some((b.get("id")?.as_str()?.to_owned(), b.get("name")?.as_str()?.to_owned()))).collect(),
            wake_ms: v.get("wakeMs").and_then(Value::as_u64),
        }
    }

    /// The words on the tile.
    pub fn headline(&self) -> &'static str {
        match self.state.as_str() {
            "absent" => "Wake up a computer",
            "asleep" => "Computer is asleep",
            "waking" => "Waking…",
            "awake" => "Computer is awake",
            "error" => "Computer can't wake",
            _ => "Checking…",
        }
    }

    /// Who else is at this computer, as a sentence; "" when no one.
    pub fn shared_with(&self, bot: &str) -> String {
        let others: Vec<&str> = self.bots.iter().filter(|(id, _)| id != bot).map(|(_, n)| n.as_str()).collect();
        if others.is_empty() { String::new() } else { format!("Shared with {}. Each has its own screen, but a screen is no wall: use separate computers for work that must stay apart.", others.join(", ")) }
    }
}

/// A computer another Bot could join.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Joinable {
    pub id: String,
    pub bots: String,
}

/// Parses `GET /v1/host/computers`, less the full ones.
pub(crate) fn joinable(v: &Value) -> Vec<Joinable> {
    let rows = v.get("computers").and_then(Value::as_array).into_iter().flatten();
    rows.filter(|c| c.get("full").and_then(Value::as_bool) != Some(true))
        .filter_map(|c| {
            let names: Vec<&str> = c.get("bots")?.as_array()?.iter().filter_map(Value::as_str).collect();
            Some(Joinable { id: c.get("id")?.as_str()?.to_owned(), bots: names.join(", ") })
        })
        .collect()
}

/// The tab's model for one Bot, kept across live patches.
#[derive(Clone, Debug, Default, PartialEq)]
struct Pane {
    bot: String,
    status: Status,
    /// the screen as a `data:` URI
    tile: Option<String>,
    seq: Option<u64>,
    join: Vec<Joinable>,
    /// a screen fetch is on its way
    fetching: bool,
    /// why a join failed, until the next try
    note: String,
}

thread_local! {
    static PANE: Signal<Pane> = Owner::detached().run(|| keep("computer.pane", Pane::default));
    /// The open recordings: each Bot's own, so one Bot's doesn't strand another's.
    static RECORDING: Signal<Recordings> = Owner::detached().run(|| keep("computer.recording", Recordings::default));
    /// A Bot delete waiting for a person's answer.
    static DELETE_ASK: Signal<Option<DeleteAsk>> = Owner::detached().run(|| keep("computer.delete-ask", || None));
}

/// The recording under way on each Bot's screen: its id, by Bot.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Recordings(std::collections::HashMap<String, u64>);

impl Recordings {
    fn of(&self, bot: &str) -> Option<u64> {
        self.0.get(bot).copied()
    }

    fn started(&mut self, bot: &str, id: u64) {
        self.0.insert(bot.to_owned(), id);
    }

    fn stopped(&mut self, bot: &str) {
        self.0.remove(bot);
    }
}

/// A Bot delete that takes more than the Bot, as a person is to read it first.
#[derive(Clone, Debug, PartialEq)]
struct DeleteAsk {
    bot: String,
    name: String,
    words: String,
}

/// What goes with Bot `name`, from its computer's status (`None`: the broker didn't say). `None`
/// back when nothing does, and the delete needs no asking.
pub(crate) fn delete_warning(status: Option<&Value>, bot: &str, name: &str) -> Option<String> {
    let Some(status) = status else {
        return Some(format!("If {name} has a computer, it goes too, with everything on it. This can't be undone."));
    };
    status.get("computer")?;
    let bots = Status::parse(status).bots;
    let others: Vec<&str> = bots.iter().filter(|(id, _)| id != bot).map(|(_, n)| n.as_str()).collect();
    Some(if others.is_empty() {
        "Its computer goes too, with everything on it: its files, downloads and recordings. This can't be undone.".to_owned()
    } else {
        format!("It leaves the computer it shares with {}, which keeps running with what {name} left on it.", others.join(", "))
    })
}

/// "Delete" on a Bot: at once, unless a computer goes with it.
pub(crate) fn ask_delete(bot: String) {
    let name = store().bots.with_untracked(|l| l.iter().find(|b| b.id == bot).map(|b| b.name.clone())).unwrap_or_else(|| "This Bot".to_owned());
    spawn_detached(async move {
        let status = client::get_any(&route(&bot, "")).await;
        match delete_warning(status.as_ref(), &bot, &name) {
            Some(words) => DELETE_ASK.with(|s| s.set(Some(DeleteAsk { bot, name, words }))),
            None => store().delete_bot(bot),
        }
    });
}

/// The question over the whole window, with its two answers.
pub(crate) fn delete_layer() -> impl IntoElement {
    when(
        || DELETE_ASK.with(|s| s.with(Option::is_some)),
        || {
            let ask = DELETE_ASK.with(|s| s.get_untracked()).unwrap_or(DeleteAsk { bot: String::new(), name: String::new(), words: String::new() });
            let bot = ask.bot.clone();
            view()
                .absolute()
                .size_full()
                .items_center()
                .justify_center()
                .bg(tc("#00000080"))
                .child(
                    view()
                        .test_id("computer-delete-ask")
                        .w(px(360.0))
                        .p(px(18.0))
                        .gap_2()
                        .rounded(px(14.0))
                        .bg(tc("#232323"))
                        .child(text(format!("Delete {}?", ask.name)).text_size(px(15.0)).font_weight(FontWeight::MEDIUM).text_color(tc("#f5f5f5")))
                        .child(text(ask.words).number_of_lines(6).text_size(px(13.0)).text_color(tc("#f5f5f5c8")))
                        .child(
                            view()
                                .flex_row()
                                .gap_2()
                                .pt(px(8.0))
                                .child(small_button("computer-delete-cancel", "Cancel", || DELETE_ASK.with(|s| s.set(None))))
                                .child(small_button("computer-delete-yes", "Delete", move || {
                                    DELETE_ASK.with(|s| s.set(None));
                                    store().delete_bot(bot.clone());
                                })),
                        ),
                )
                .into_element()
        },
    )
}

fn pane_signal() -> Signal<Pane> {
    PANE.with(|s| *s)
}

fn update(bot: &str, f: impl FnOnce(&mut Pane)) {
    pane_signal().update(|p| {
        if p.bot == bot {
            f(p);
        }
    });
}

fn route(bot: &str, rest: &str) -> String {
    format!("/v1/host/computer/bot/{}{rest}", client::urlenc(bot))
}

/// The key a Bot's computer opens under.
pub(crate) fn key_of(bot: &str) -> String {
    format!("{KEY_PREFIX}{bot}")
}

/// The Bot of a computer's key; `None` for any other app's.
pub(crate) fn bot_of_key(key: &str) -> Option<String> {
    key.strip_prefix(KEY_PREFIX).filter(|b| !b.is_empty()).map(str::to_owned)
}

async fn refresh(bot: String) {
    let Some(v) = client::get_any(&route(&bot, "")).await else { return };
    let status = Status::parse(&v);
    let want_list = status.state == "absent";
    let awake = status.state == "awake";
    update(&bot, |p| {
        if !awake {
            p.tile = None;
            p.seq = None;
        }
        p.status = status;
    });
    if want_list {
        let join = client::get("/v1/host/computers").await.map(|v| joinable(&v)).unwrap_or_default();
        update(&bot, |p| p.join = join);
    }
    if awake {
        refresh_tile(bot).await;
    }
}

/// The tile, when the screen changed: the broker waits a moment and says 204 when it didn't.
async fn refresh_tile(bot: String) {
    let (busy, seq) = pane_signal().with_untracked(|p| (p.fetching, p.seq));
    if busy {
        return;
    }
    update(&bot, |p| p.fetching = true);
    let path = match seq {
        Some(s) => format!("{}?after={s}", route(&bot, "/screen")),
        None => route(&bot, "/screen"),
    };
    let got = client::get_bytes(&path, "x-computer-seq").await;
    update(&bot, |p| {
        p.fetching = false;
        if let Some((200, seq, body)) = got {
            p.seq = seq.and_then(|s| s.parse().ok());
            p.tile = Some(nsh_core::platform::clipboard::data_uri("image/png", &body));
        }
    });
}

/// A person clicked the computer.
fn wake(bot: String) {
    update(&bot, |p| p.status.state = "waking".to_owned());
    spawn_detached(async move {
        if let Some(v) = client::post_any(&route(&bot, "/wake"), json!({})).await {
            let status = Status::parse(&v);
            update(&bot, |p| p.status = status);
        }
    });
}

fn sleep(bot: String) {
    spawn_detached(async move {
        let _ = client::post_any(&route(&bot, "/sleep"), json!({})).await;
        refresh(bot).await;
    });
}

/// Joins `computer`, and wakes it only if that worked: with no seat, a wake makes a new computer.
fn attach(bot: String, computer: String) {
    update(&bot, |p| p.note.clear());
    spawn_detached(async move {
        let reply = client::post_any(&route(&bot, "/attach"), json!({"computer": computer})).await;
        match refusal(reply.as_ref()) {
            Some(why) => update(&bot, |p| p.note = why),
            None => wake(bot),
        }
    });
}

/// The sentence of a broker's refusal: `{ok: false, message}`, or no answer. `None` for a yes.
pub(crate) fn refusal(reply: Option<&Value>) -> Option<String> {
    let Some(v) = reply else { return Some("The broker didn't answer.".to_owned()) };
    (v.get("ok").and_then(Value::as_bool) == Some(false)).then(|| v.get("message").and_then(Value::as_str).unwrap_or("The computer said no.").to_owned())
}

fn leave(bot: String) {
    spawn_detached(async move {
        let _ = client::post_any(&route(&bot, "/detach"), json!({})).await;
        refresh(bot).await;
    });
}

fn open_full(bot: &str, name: &str) {
    store().open_mini_token(&key_of(bot), &format!("{name}'s computer"), "");
}

/// The Computer tab's body. Its poll runs while it is shown and stops with it.
pub(crate) fn pane() -> impl IntoElement {
    fn tick() {
        if store().detail_tab.get_untracked() != "computer" {
            return;
        }
        let Some(b) = store().active_bot() else { return };
        if pane_signal().with_untracked(|p| p.bot != b.id) {
            pane_signal().set(Pane { bot: b.id.clone(), ..Pane::default() });
        }
        spawn_detached(refresh(b.id));
    }
    tick();
    set_interval(POLL, tick);
    view().test_id("computer-pane").child(dynamic(|| {
        let Some(b) = store().active_bot() else {
            return note("A computer belongs to a Bot. Open a Bot to use one.".to_owned()).into_element();
        };
        let p = pane_signal().get();
        body(&b.id, &b.name, &b.shape, &p).into_element()
    }))
}

fn note(words: String) -> impl IntoElement {
    text(words).text_size(px(12.0)).line_height(px(16.0)).text_color(theme::MUTED).pt(px(10.0))
}

fn small_button(id: &'static str, words: &'static str, press: impl Fn() + 'static) -> impl IntoElement {
    view()
        .test_id(id)
        .label(words)
        .h(px(28.0))
        .px(px(12.0))
        .rounded(px(14.0))
        .items_center()
        .justify_center()
        .bg(tc("#f5f5f50f"))
        .child(text(words).text_size(px(12.0)).text_color(theme::TEXT))
        .on_press(press)
}

fn body(bot: &str, name: &str, shape: &str, p: &Pane) -> impl IntoElement {
    let awake = p.status.state == "awake";
    let (b, n) = (bot.to_owned(), name.to_owned());
    let tile = view().test_id("computer-tile").h(px(180.0)).rounded(px(12.0)).bg(theme::MAIN).items_center().justify_center().overflow_hidden();
    let tile = match (&p.tile, awake) {
        (Some(uri), true) => tile
            .label("Open computer")
            .child(image(uri.clone()).size_full().object_fit(ResizeMode::Contain))
            .on_press(move || open_full(&b, &n)),
        _ => {
            let words = p.status.headline();
            let press = (p.status.state != "waking").then(|| b.clone());
            let tile = tile.label("Wake computer").child(sleeping_avatar(shape)).child(text(words).text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(theme::TEXT).pt(px(6.0)));
            match press {
                Some(bot) if !awake => tile.on_press(move || wake(bot.clone())),
                _ => tile,
            }
        }
    };
    let mut out = view().child(tile).child(text(format!("{name}'s screen")).text_size(px(12.0)).text_color(theme::MUTED).pt(px(10.0)));
    for words in [&p.status.message, &p.note].into_iter().filter(|w| !w.is_empty()) {
        out = out.child(view().test_id("computer-problem").child(note(words.clone())));
    }
    let shared = p.status.shared_with(bot);
    if !shared.is_empty() {
        out = out.child(note(shared));
    }
    if let Some(ms) = p.status.wake_ms.filter(|_| awake) {
        out = out.child(note(format!("Woke in {:.1} s", ms as f64 / 1000.0)));
    }
    if awake {
        let (b1, n1, b2) = (bot.to_owned(), name.to_owned(), bot.to_owned());
        out = out.child(
            view()
                .flex_row()
                .gap_2()
                .pt(px(10.0))
                .child(small_button("computer-open", "Open", move || open_full(&b1, &n1)))
                .child(small_button("computer-sleep", "Sleep", move || sleep(b2.clone()))),
        );
    }
    if p.status.bots.len() > 1 {
        let b = bot.to_owned();
        out = out.child(view().flex_row().pt(px(10.0)).child(small_button("computer-leave", "Leave this computer", move || leave(b.clone()))));
    }
    if p.status.state == "absent" {
        for j in &p.join {
            let (b, id) = (bot.to_owned(), j.id.clone());
            out = out.child(
                view()
                    .test_id("computer-join")
                    .label("Share a computer")
                    .pt(px(10.0))
                    .child(text(format!("Share {}'s computer", j.bots)).text_size(px(12.0)).text_color(theme::TEXT))
                    .on_press(move || attach(b.clone(), id.clone())),
            );
        }
    }
    out
}

fn sleeping_avatar(shape: &str) -> impl IntoElement {
    crate::avatar::sleeping_blob(shape, 33.0, "#f5f5f58a")
}

/// The open viewer: a URL once the computer is awake and the broker gave a ticket.
async fn ticket(bot: &str) -> Result<String, String> {
    match client::post_any(&route(bot, "/open"), json!({})).await {
        Some(v) => match v.get("url").and_then(Value::as_str) {
            Some(url) => Ok(url.to_owned()),
            None => Err(v.get("message").and_then(Value::as_str).unwrap_or("The computer can't show its screen.").to_owned()),
        },
        None => Err("The broker didn't answer.".to_owned()),
    }
}

/// The full-screen body: the live desktop, or why not and a way to wake it. A pin opens here too,
/// so a sleeping computer is woken from here as well.
pub(crate) fn live_view(bot: String) -> impl IntoElement {
    let url: Signal<String> = signal(String::new());
    let state: Signal<Status> = signal(Status::default());
    let asking = signal(false);
    let look = {
        let bot = bot.clone();
        move || {
            let bot = bot.clone();
            spawn_detached(async move {
                let Some(v) = client::get_any(&route(&bot, "")).await else { return };
                let now = Status::parse(&v);
                let awake = now.state == "awake";
                state.set(now.clone());
                if !awake {
                    if !url.get_untracked().is_empty() {
                        url.set(String::new());
                    }
                } else if url.get_untracked().is_empty() && !asking.get_untracked() {
                    asking.set(true);
                    match ticket(&bot).await {
                        Ok(u) => url.set(u),
                        Err(message) => state.set(Status { state: "error".to_owned(), message, ..now }),
                    }
                    asking.set(false);
                }
            });
        }
    };
    look();
    set_interval(POLL, look);
    let press = bot.clone();
    view()
        .test_id("computer-live")
        .flex_1()
        .child(
            web_view(move || {
                let u = url.get();
                if u.is_empty() { "about:blank".to_owned() } else { u }
            })
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .scrollable(false)
            .opacity(move || if url.with(String::is_empty) { 0.0f32 } else { 1.0 }),
        )
        .child(when(
            move || url.with(String::is_empty),
            move || {
                let press = press.clone();
                view()
                    .test_id("computer-live-wake")
                    .label("Wake computer")
                    .absolute()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .px_6()
                    .child(text(move || state.with(|s| s.headline().to_owned())).text_size(px(15.0)).font_weight(FontWeight::MEDIUM).text_color(tc("#f5f5f5")))
                    .child(text(move || state.with(|s| s.message.clone())).number_of_lines(6).text_size(px(13.0)).text_color(tc("#f5f5f5c8")).pt(px(8.0)))
                    .on_press(move || {
                        if !matches!(state.with_untracked(|s| s.state.clone()).as_str(), "waking" | "awake") {
                            state.update(|s| s.state = "waking".to_owned());
                            wake(press.clone());
                        }
                    })
                    .into_element()
            },
        ))
}

/// Whether a Stop's answer says the recording ended.
pub(crate) fn stopped(reply: Option<&Value>) -> bool {
    reply.and_then(|v| v.get("ok")).and_then(Value::as_bool) == Some(true)
}

/// Record, beside Pin: on demand, and the MP4 stays on the computer.
pub(crate) fn record_button(key: &str) -> impl IntoElement {
    let bot = bot_of_key(key).unwrap_or_default();
    let recording = RECORDING.with(|s| *s);
    let b = bot.clone();
    let on = move || recording.with(|r| r.of(&b).is_some());
    let shown = on.clone();
    view()
        .test_id("computer-record")
        .label("Record")
        .h(px(32.0))
        .px(px(12.0))
        .rounded(px(16.0))
        .items_center()
        .justify_center()
        .bg(tc("#f5f5f50f"))
        .child(text(move || if shown() { "Stop".to_owned() } else { "Record".to_owned() }).text_size(px(13.0)).text_color(tc("#f5f5f5a8")))
        .on_press(move || {
            let bot = bot.clone();
            let was = recording.with_untracked(|r| r.of(&bot));
            spawn_detached(async move {
                let path = route(&bot, "/record");
                match was {
                    Some(id) => {
                        // a stop that failed leaves it recording: the button stays, to try again
                        let reply = client::post_any(&path, json!({"action": "stop", "id": id})).await;
                        if stopped(reply.as_ref()) {
                            recording.update(|r| r.stopped(&bot));
                        }
                    }
                    None => {
                        let id = client::post_any(&path, json!({"action": "start"})).await.and_then(|v| v.get("id").and_then(Value::as_u64));
                        if let Some(id) = id {
                            recording.update(|r| r.started(&bot, id));
                        }
                    }
                }
            });
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_state_has_its_words() {
        let s = |state: &str| Status { state: state.to_owned(), ..Status::default() }.headline();
        assert_eq!(s("asleep"), "Computer is asleep");
        assert_eq!(s("waking"), "Waking…");
        assert_eq!(s("awake"), "Computer is awake");
        assert_eq!(s("error"), "Computer can't wake");
    }

    #[test]
    fn the_status_reads_the_brokers_words() {
        let v = json!({"state": "error", "message": "Docker isn't running.", "wakeMs": 1200, "bots": [{"id": "b_1", "name": "Ann"}, {"id": "b_2", "name": "Bea"}]});
        let s = Status::parse(&v);
        assert_eq!((s.state.as_str(), s.message.as_str(), s.wake_ms), ("error", "Docker isn't running.", Some(1200)));
        assert_eq!(s.shared_with("b_1"), "Shared with Bea. Each has its own screen, but a screen is no wall: use separate computers for work that must stay apart.");
        assert_eq!(Status { bots: vec![("b_1".into(), "Ann".into())], ..Status::default() }.shared_with("b_1"), "");
    }

    #[test]
    fn a_computers_key_names_its_bot_and_no_other_app_does() {
        assert_eq!(bot_of_key(&key_of("b_1")).as_deref(), Some("b_1"));
        assert_eq!(bot_of_key("mcp:apps/hello"), None);
        assert_eq!(bot_of_key("computer:"), None);
    }

    #[test]
    fn a_failed_join_is_a_sentence_and_stops_there() {
        assert_eq!(refusal(Some(&json!({"ok": false, "message": "A computer has room for 4 Bots."}))).as_deref(), Some("A computer has room for 4 Bots."));
        assert_eq!(refusal(None).as_deref(), Some("The broker didn't answer."));
        assert_eq!(refusal(Some(&json!({"state": "awake", "computer": "c"}))), None, "a status is a yes");
    }

    #[test]
    fn only_a_yes_ends_a_recording() {
        assert!(stopped(Some(&json!({"ok": true, "path": "x.mp4"}))));
        assert!(!stopped(Some(&json!({"ok": false, "message": "no"}))));
        assert!(!stopped(None));
    }

    #[test]
    fn each_bot_keeps_its_own_recording() {
        let mut r = Recordings::default();
        r.started("ann", 1);
        r.started("bea", 2);
        assert_eq!((r.of("ann"), r.of("bea")), (Some(1), Some(2)), "Bea's start stranded Ann's");
        r.stopped("bea");
        assert_eq!((r.of("ann"), r.of("bea")), (Some(1), None));
    }

    #[test]
    fn deleting_a_bot_says_what_goes_with_it() {
        let alone = json!({"computer": "c", "bots": [{"id": "1", "name": "Ann"}]});
        let words = delete_warning(Some(&alone), "1", "Ann").unwrap();
        assert!(words.contains("computer goes too") && words.contains("files") && words.contains("recordings") && words.contains("can't be undone"), "{words}");
        let shared = json!({"computer": "c", "bots": [{"id": "1", "name": "Ann"}, {"id": "2", "name": "Bea"}]});
        assert!(delete_warning(Some(&shared), "1", "Ann").unwrap().contains("shares with Bea"));
        let twins = json!({"computer": "c", "bots": [{"id": "1", "name": "Ann"}, {"id": "2", "name": "Ann"}]});
        assert!(delete_warning(Some(&twins), "1", "Ann").unwrap().contains("shares with Ann"), "two Bots can share a name");
        assert_eq!(delete_warning(Some(&json!({"state": "absent"})), "1", "Ann"), None, "no computer, nothing to warn of");
        assert!(delete_warning(None, "1", "Ann").unwrap().contains("If Ann has a computer"), "not knowing is a warning");
    }

    #[test]
    fn a_full_computer_isnt_offered() {
        let v = json!({"computers": [{"id": "c1", "bots": ["Ann", "Bea"], "full": false}, {"id": "c2", "bots": ["X"], "full": true}]});
        assert_eq!(joinable(&v), [Joinable { id: "c1".into(), bots: "Ann, Bea".into() }]);
    }
}
