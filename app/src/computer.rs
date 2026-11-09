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
//!
//! A ticket says watch or drive. A viewer watches until a person presses Take control: their input
//! reaches the desktop and the agent waits while they use it. When a Bot asks a person to take over,
//! the viewer drives for as long as that lasts, and Hand back ends it. The broker's status says
//! what the viewer may be, so when a hand-over ends the ticket is swapped here, however it ended.
//!
//! A person can connect a remote computer instead ([`remote_form`]). What they type goes to the
//! broker and stays in memory here only until the form closes. Its key is a file's path, so the
//! key itself is never on screen. The tab says what's different, in words: nothing walls off that
//! machine, sleeping is disconnecting, and removing forgets the connection.
//!
//! With no computer, the tab says where this Bot's next one would be made, and Set up a computer
//! opens the chat's pick-a-home card here (`cards/computer_setup.rs`, homes in `homes.rs`).

use nsh_base::input;
use nsh_core::prelude::*;
use serde_json::{json, Value};

use crate::client;
use crate::homes;
use crate::live::{keep, store};
use crate::store::{spawn_detached, CtxItem};
use crate::theme::{self, tc};

const KEY_PREFIX: &str = "computer:";
const POLL: std::time::Duration = std::time::Duration::from_secs(2);
/// How long "Woke in" stays once it shows.
const WOKE_FOR: std::time::Duration = std::time::Duration::from_secs(10);
const LEFT_THE_GATE: &str = "This computer's page tried to take this view somewhere else, so it was closed. Click to open it again.";

/// What the broker says about this Bot's computer.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Status {
    /// `absent`, `asleep`, `waking`, `awake` or `error`
    pub state: String,
    pub message: String,
    /// the Bots at this computer, this one included: (id, name)
    pub bots: Vec<(String, String)>,
    pub wake_ms: Option<u64>,
    /// the open viewer's mode, `watch` or `drive`; "" when none is open
    pub viewer: String,
    /// a run has this screen
    pub working: bool,
    pub takeover: Option<Takeover>,
    pub remote: Option<Remote>,
    /// the jobs on this computer, by any Bot at it
    pub jobs: Vec<Job>,
    /// where this Bot's next new computer would be made
    pub home: Option<homes::Next>,
}

/// A command a Bot left running on this computer.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Job {
    pub name: String,
    pub bot: String,
    /// its computer sleeps, so it's stopped where it was
    pub frozen: bool,
    pub seconds_left: u64,
}

impl Job {
    /// The tab's line: how it stands now, and nothing it can't say. A remote computer's job isn't
    /// stopped by a disconnect, and nothing here can see it then.
    fn words(&self, remote: bool) -> String {
        let job = if self.name.is_empty() { format!("{}'s job", self.bot) } else { format!("{}'s job \"{}\"", self.bot, self.name) };
        match (self.frozen, remote) {
            (true, true) => format!("{job}: not connected, so how it stands isn't known."),
            (true, false) => format!("{job} is frozen while this computer sleeps."),
            (false, _) => format!("{job} is running · {} left", span(self.seconds_left)),
        }
    }

    fn parse(v: &Value) -> Option<Self> {
        Some(Self { name: v.get("name")?.as_str()?.to_owned(), bot: v.get("bot")?.as_str()?.to_owned(), frozen: v.get("frozen")?.as_bool()?, seconds_left: v.get("secondsLeft")?.as_u64()? })
    }
}

/// `3 h 59 min`, or `12 min`; under a minute, `under a minute`.
fn span(seconds: u64) -> String {
    match (seconds / 3600, seconds % 3600 / 60) {
        (0, 0) => "under a minute".to_owned(),
        (0, m) => format!("{m} min"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// A computer a person connected.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Remote {
    /// a box: a screen and a viewer. Otherwise commands and files alone
    pub screen: bool,
    pub place: String,
}

impl Remote {
    fn parse(v: &Value) -> Option<Self> {
        Some(Self { screen: v.get("screen")?.as_bool()?, place: v.get("where")?.as_str()?.to_owned() })
    }

    /// What's different about it, for the tab.
    fn words(&self) -> String {
        let what = if self.screen { "" } else { " It has no screen: commands and files only." };
        format!("Remote computer {}. The AI acts on that machine with whatever access you gave it, and nothing walls it off.{what} Disconnecting and removing don't touch the machine.", self.place)
    }
}

/// The screen is with a person, because its Bot asked.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Takeover {
    pub reason: String,
    pub seconds_left: u64,
}

impl Takeover {
    fn parse(v: &Value) -> Option<Self> {
        Some(Self { reason: v.get("reason")?.as_str()?.to_owned(), seconds_left: v.get("secondsLeft")?.as_u64()? })
    }

    /// `4:05`.
    fn left(&self) -> String {
        format!("{}:{:02}", self.seconds_left / 60, self.seconds_left % 60)
    }
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
            viewer: text("viewer"),
            working: v.get("working").and_then(Value::as_bool).unwrap_or(false),
            takeover: v.get("takeover").and_then(Takeover::parse),
            remote: v.get("remote").and_then(Remote::parse),
            jobs: v.get("jobs").and_then(Value::as_array).into_iter().flatten().filter_map(Job::parse).collect(),
            home: v.get("home").and_then(homes::Next::parse),
        }
    }

    /// Whether there's a screen to show.
    fn has_screen(&self) -> bool {
        self.remote.as_ref().is_none_or(|r| r.screen)
    }

    /// The words on the tile.
    pub fn headline(&self) -> &'static str {
        if self.remote.is_some() {
            return match self.state.as_str() {
                "asleep" => "Not connected",
                "waking" => "Connecting…",
                "awake" => "Connected",
                "error" => "Can't connect",
                _ => "Checking…",
            };
        }
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

/// The mode a viewer should get a ticket for now, given the one it `holds` ("" for none) and
/// whether a person `wants_to_drive`: `None` leaves it as it is. What the broker says wins, so a
/// session it closed is never taken for one still open, and an ended hand-over leaves no way to go
/// on driving.
pub(crate) fn next_ticket(status: &Status, holds: &str, wants_to_drive: bool) -> Option<&'static str> {
    if status.state != "awake" {
        return None;
    }
    let want = if status.takeover.is_some() || wants_to_drive { "drive" } else { "watch" };
    (holds != want || status.viewer != holds).then_some(want)
}

/// What the bar says of the open viewer.
pub(crate) fn mode_words(status: &Status) -> String {
    match (&status.takeover, status.viewer.as_str()) {
        (Some(t), _) => format!("{} · {} left", t.reason, t.left()),
        (None, "drive") => "You're driving. The agent waits while you do.".to_owned(),
        (None, "watch") if status.working => "Watching the agent work.".to_owned(),
        (None, "watch") => "Watching. Your clicks don't reach the screen.".to_owned(),
        _ => String::new(),
    }
}

/// What the menu's Sleep says: it wins over a job, so it says what that does to one. A remote
/// computer's Disconnect stops nothing there.
pub(crate) fn sleep_words(status: &Status) -> &'static str {
    match (status.remote.is_some(), status.jobs.iter().filter(|j| !j.frozen).count()) {
        (true, _) => "Disconnect",
        (false, 0) => "Sleep",
        (false, 1) => "Sleep, and freeze its job",
        (false, _) => "Sleep, and freeze its jobs",
    }
}

/// What the bar's button says.
pub(crate) fn button_words(status: &Status) -> &'static str {
    match (&status.takeover, status.viewer.as_str()) {
        (Some(_), _) => "Hand back",
        (None, "drive") => "Stop driving",
        _ => "Take control",
    }
}

/// A computer another Bot could join.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Joinable {
    pub id: String,
    pub bots: String,
    pub remote: bool,
}

impl Joinable {
    fn words(&self) -> String {
        if self.remote {
            format!("Share {}'s remote computer. The AI acts on that machine, and nothing walls it off.", self.bots)
        } else {
            format!("Share {}'s computer", self.bots)
        }
    }
}

/// Parses `GET /v1/host/computers`, less the full ones.
pub(crate) fn joinable(v: &Value) -> Vec<Joinable> {
    let rows = v.get("computers").and_then(Value::as_array).into_iter().flatten();
    rows.filter(|c| c.get("full").and_then(Value::as_bool) != Some(true))
        .filter_map(|c| {
            let names: Vec<&str> = c.get("bots")?.as_array()?.iter().filter_map(Value::as_str).collect();
            Some(Joinable { id: c.get("id")?.as_str()?.to_owned(), bots: names.join(", "), remote: c.get("remote").and_then(Value::as_bool).unwrap_or(false) })
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
    /// The Bots whose screen a person pressed Take control on.
    static DRIVING: Signal<std::collections::HashSet<String>> = Owner::detached().run(|| keep("computer.driving", Default::default));
    /// What the viewer open full screen last heard, with whose it is.
    static SHOWN: Signal<(String, Status)> = Owner::detached().run(|| keep("computer.shown", Default::default));
    /// Each Bot's last wake seen on the tab, in ms, and since when its "Woke in" shows; `None` once
    /// that's gone. The broker says how long the last wake took for as long as the computer is up.
    static WOKE: Signal<std::collections::HashMap<String, (u64, Option<std::time::Instant>)>> = Owner::detached().run(|| keep("computer.woke", Default::default));
    /// The form for a remote computer. Secrets are in here for as long as it's open.
    static FORM: Form = Owner::detached().run(Form::new);
    /// The tab's Set up a computer is open.
    static SETUP: Signal<bool> = Owner::detached().run(|| signal(false));
}

/// How far the form has got.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Stage {
    #[default]
    Closed,
    Editing,
    /// the far end is being asked
    Busy,
    /// a person is to check this before anything is sent. `vouched`: the public roots take it
    Trust { fingerprint: String, pin: String, vouched: bool },
    /// it works: `what` it is, and `consent` for them to read
    Ready { what: String, consent: String },
}

const BOX_READY: &str = "The way in works. The box's token and viewer password haven't been sent: they go when you say yes, and the box itself is checked then.";

/// What the form is at once a field has changed: nothing tested is what's typed now.
pub(crate) fn stage_after_edit(stage: &Stage) -> Stage {
    match stage {
        Stage::Closed => Stage::Closed,
        _ => Stage::Editing,
    }
}

/// What the broker's test said, for the form.
pub(crate) fn stage_of(reply: Option<&Value>) -> Result<Stage, String> {
    let Some(v) = reply else { return Err("The broker didn't answer.".to_owned()) };
    let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    if v.get("ok").and_then(Value::as_bool) == Some(true) {
        let system = v.get("system").and_then(Value::as_str).unwrap_or("a plain SSH host");
        let what = if v.get("screen").and_then(Value::as_bool) == Some(true) { BOX_READY.to_owned() } else { format!("{system} answered over SSH. There's no screen: commands and files only.") };
        return Ok(Stage::Ready { what, consent: text("consent") });
    }
    if text("code") == "trust" {
        return Ok(Stage::Trust { fingerprint: text("fingerprint"), pin: text("pin"), vouched: v.get("vouched").and_then(Value::as_bool).unwrap_or(false) });
    }
    Err(v.get("message").and_then(Value::as_str).unwrap_or("That computer can't be used.").to_owned())
}

/// The form's fields, each its own signal so that typing redraws nothing around it.
#[derive(Clone, Copy)]
struct Form {
    stage: Signal<Stage>,
    url: Signal<String>,
    key_file: Signal<String>,
    token: Signal<String>,
    viewer: Signal<String>,
    pin: Signal<String>,
    note: Signal<String>,
    /// how many edits there have been: a Test's answer for older ones is no answer
    edits: Signal<u64>,
}

impl Form {
    fn new() -> Self {
        Self { stage: signal(Stage::Closed), url: signal(String::new()), key_file: signal(String::new()), token: signal(String::new()), viewer: signal(String::new()), pin: signal(String::new()), note: signal(String::new()), edits: signal(0) }
    }

    fn body(&self) -> Value {
        json!({"url": self.url.get_untracked(), "keyFile": self.key_file.get_untracked(), "token": self.token.get_untracked(), "viewerPassword": self.viewer.get_untracked(), "pin": self.pin.get_untracked()})
    }

    /// A person changed a field. A key trusted for one address says nothing of another.
    fn edited(&self, address: bool) {
        self.edits.update(|n| *n += 1);
        if address {
            self.pin.set(String::new());
        }
        let next = stage_after_edit(&self.stage.get_untracked());
        self.stage.set(next);
    }

    /// Back to nothing: no secret stays once the form is done with.
    fn clear(&self) {
        self.edits.update(|n| *n += 1);
        for field in [self.url, self.key_file, self.token, self.viewer, self.pin, self.note] {
            field.set(String::new());
        }
        self.stage.set(Stage::Closed);
    }
}

fn form() -> Form {
    FORM.with(|f| *f)
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
    let parsed = Status::parse(status);
    let others: Vec<&str> = parsed.bots.iter().filter(|(id, _)| id != bot).map(|(_, n)| n.as_str()).collect();
    Some(if others.is_empty() && parsed.remote.is_some() {
        "Its connection to the remote computer goes too. Nothing on that machine changes.".to_owned()
    } else if others.is_empty() {
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
    let mut wake = None;
    pane_signal().update(|p| {
        if p.bot == bot {
            f(p);
            // a pane just reset has heard nothing, and that's no sleep
            wake = (!p.status.state.is_empty()).then(|| p.status.wake_ms.filter(|_| p.status.state == "awake"));
        }
    });
    if let Some(wake) = wake {
        note_wake(bot, wake);
    }
}

/// Starts "Woke in" for a wake not seen yet, and forgets the last one when `wake` is `None`, so the
/// next shows even if it took as long.
fn note_wake(bot: &str, wake: Option<u64>) {
    let woke = WOKE.with(|s| *s);
    if woke.with_untracked(|w| w.get(bot).map(|seen| seen.0)) == wake {
        return;
    }
    let Some(ms) = wake else {
        return woke.update(|w| {
            w.remove(bot);
        });
    };
    let since = std::time::Instant::now();
    woke.update(|w| {
        w.insert(bot.to_owned(), (ms, Some(since)));
    });
    let bot = bot.to_owned();
    spawn_detached(async move {
        timer(WOKE_FOR).await;
        woke.update(|w| {
            if let Some(seen) = w.get_mut(&bot).filter(|seen| seen.1 == Some(since)) {
                seen.1 = None;
            }
        });
    });
}

/// Whether `bot`'s "Woke in" still shows.
fn woke_shows(bot: &str) -> bool {
    WOKE.with(|s| s.with(|w| w.get(bot).is_some_and(|seen| seen.1.is_some())))
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
    let show_tile = awake && status.has_screen();
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
    if show_tile {
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
pub(crate) fn wake(bot: String) {
    update(&bot, |p| p.status.state = "waking".to_owned());
    spawn_detached(async move {
        if let Some(v) = form_post(&route(&bot, "/wake"), json!({})).await {
            let status = Status::parse(&v);
            update(&bot, |p| p.status = status);
        }
    });
}

pub(crate) fn sleep(bot: String) {
    spawn_detached(async move {
        let _ = form_post(&route(&bot, "/sleep"), json!({})).await;
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

/// What a test's broker answers, in order, and what it was sent. A headless engine has no network.
#[cfg(test)]
pub(crate) static CANNED: std::sync::Mutex<Canned> = std::sync::Mutex::new((Vec::new(), Vec::new()));
/// The answers still to give, then each path and body that was sent.
#[cfg(test)]
pub(crate) type Canned = (Vec<Value>, Vec<(String, Value)>);

#[cfg(not(test))]
async fn form_post(path: &str, body: Value) -> Option<Value> {
    client::post_any(path, body).await
}

/// A test's broker takes its time while this is set.
#[cfg(test)]
pub(crate) static SLOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
async fn form_post(path: &str, body: Value) -> Option<Value> {
    while SLOW.load(std::sync::atomic::Ordering::SeqCst) {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let mut canned = CANNED.lock().unwrap();
    canned.1.push((path.to_owned(), body));
    (!canned.0.is_empty()).then(|| canned.0.remove(0))
}

/// Asks the broker about what's in the form, with what's been trusted so far.
fn test_remote() {
    let f = form();
    let bot = pane_signal().with_untracked(|p| p.bot.clone());
    f.note.set(String::new());
    f.stage.set(Stage::Busy);
    let asked_at = f.edits.get_untracked();
    spawn_detached(async move {
        let reply = form_post(&route(&bot, "/remote/test"), f.body()).await;
        if f.edits.get_untracked() != asked_at {
            return;
        }
        match stage_of(reply.as_ref()) {
            Ok(stage) => f.stage.set(stage),
            Err(why) => {
                f.note.set(why);
                f.stage.set(Stage::Editing);
            }
        }
    });
}

/// A person read the words and said yes.
fn add_remote(bot: String) {
    let f = form();
    f.note.set(String::new());
    f.stage.set(Stage::Busy);
    spawn_detached(async move {
        let mut body = f.body();
        body["consent"] = json!(true);
        let reply = form_post(&route(&bot, "/remote"), body).await;
        match refusal(reply.as_ref()) {
            Some(why) => {
                f.note.set(why);
                f.stage.set(Stage::Editing);
            }
            None => {
                f.clear();
                refresh(bot).await;
            }
        }
    });
}

/// Forgets the connection. Nothing on the machine changes.
fn remove_remote(bot: String) {
    spawn_detached(async move {
        let reply = client::post_any(&route(&bot, "/remote/remove"), json!({})).await;
        let why = refusal(reply.as_ref());
        update(&bot, |p| p.note = why.clone().unwrap_or_default());
        refresh(bot).await;
    });
}

fn leave(bot: String) {
    spawn_detached(async move {
        let _ = client::post_any(&route(&bot, "/detach"), json!({})).await;
        refresh(bot).await;
    });
}

/// What Remove says at `bot`: its button, and who else it takes off the computer.
pub(crate) fn remove_words(status: &Status, bot: &str) -> (&'static str, Option<String>) {
    let others: Vec<&str> = status.bots.iter().filter(|(id, _)| id != bot).map(|(_, n)| n.as_str()).collect();
    if others.is_empty() {
        ("Remove this computer", None)
    } else {
        ("Remove it for every Bot here", Some(format!("Removing it takes {} off it too. The machine isn't touched.", others.join(", "))))
    }
}

/// Whether `page`, starting to load in a viewer, is still on the gate `ticket` is for. Only a web
/// view rule could stop it first: this is how a viewer knows a page has left.
pub(crate) fn stays_on_gate(ticket: &str, page: &str) -> bool {
    let origin = |url: &str| url.strip_prefix("http://").and_then(|r| r.split('/').next()).map(str::to_owned);
    page.is_empty() || page == "about:blank" || (origin(ticket).is_some() && origin(ticket) == origin(page))
}

/// What a viewer shows of `status`: after a page left the gate, that, until a person clicks, however
/// well the computer itself is.
pub(crate) fn after_leaving(status: Status, left: bool) -> Status {
    if left {
        Status { state: "error".to_owned(), message: LEFT_THE_GATE.to_owned(), viewer: String::new(), ..status }
    } else {
        status
    }
}

/// Puts what `bot`'s viewer last heard on its bar.
pub(crate) fn show_status(bot: &str, status: Status) {
    SHOWN.with(|s| s.set((bot.to_owned(), status)));
}

/// Whether a person pressed Take control on `bot`'s screen.
#[cfg(test)]
pub(crate) fn wants_to_drive(bot: &str) -> bool {
    DRIVING.with(|s| s.with(|d| d.contains(bot)))
}

/// The person is done with the screen their Bot gave them.
fn hand_back(bot: String) {
    spawn_detached(async move {
        let reply = client::post_any(&route(&bot, "/handback"), json!({})).await;
        let why = refusal(reply.as_ref());
        update(&bot, |p| p.note = why.clone().unwrap_or_default());
        refresh(bot).await;
    });
}

/// The bar's button: Take control and Stop driving ask for the other ticket, and Hand back ends the
/// hand-over.
fn press_drive(bot: &str, status: &Status) {
    if status.takeover.is_some() {
        return hand_back(bot.to_owned());
    }
    let on = status.viewer != "drive";
    DRIVING.with(|s| {
        s.update(|d| {
            if on {
                d.insert(bot.to_owned());
            } else {
                d.remove(bot);
            }
        })
    });
}

pub(crate) fn open_full(bot: &str, name: &str) {
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
            // what was typed and agreed to was for the Bot this tab was on
            form().clear();
            SETUP.with(|s| s.set(false));
        }
        spawn_detached(refresh(b.id));
    }
    tick();
    set_interval(POLL, tick);
    view()
        .test_id("computer-pane")
        .child(dynamic(|| {
            let Some(b) = store().active_bot() else {
                return note("A computer belongs to a Bot. Open a Bot to use one.".to_owned()).into_element();
            };
            let p = pane_signal().get();
            body(&b.id, &b.name, &b.shape, &p).into_element()
        }))
        // outside the part a poll redraws, so a field keeps its focus
        .child(when(|| pane_signal().with(|p| p.status.state == "absent"), || remote_form().into_element()))
}

/// A field of the form.
fn field(id: &'static str, hint: &'static str, masked: bool, value: Signal<String>) -> impl IntoElement {
    let address = id == "computer-remote-url";
    view().h(px(32.0)).rounded(px(8.0)).bg(theme::HOVER).px(px(10.0)).justify_center().child(
        input()
            .test_id(id)
            .label(hint)
            .value(move || value.get())
            .placeholder(hint)
            .placeholder_color(|| color(theme::GHOST))
            .masked(masked)
            .text_size(px(12.0))
            .text_color(theme::TEXT)
            .h(px(32.0))
            .on_change(move |t: &str| {
                // a field set from here isn't a person's edit
                if value.with_untracked(|v| v != t) {
                    value.set(t.to_owned());
                    form().edited(address);
                }
            }),
    )
}

/// Whether `url` is a box behind TLS: a certificate answers for it, and no SSH key comes into it.
pub(crate) fn over_https(url: &str) -> bool {
    url.trim().get(..8).is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
}

/// What a person is asked to check. Over https that's a certificate, which may be on a server in
/// front of the box and not on the box. A public authority's word is for the name, not the machine,
/// and a pinned certificate has to be trusted again when it renews.
pub(crate) fn trust_words(fingerprint: &str, vouched: bool, https: bool) -> String {
    let vouched = if vouched { " A public authority vouches for this name. When its certificate renews, you'd remove this computer and add it again." } else { "" };
    let check = if https { "Check this against the certificate on the server that answers at this address" } else { "Check that this is that machine's key, on the machine itself," };
    format!("Nothing has been sent yet. {check} before you trust it: {fingerprint}.{vouched}")
}

/// What the form shows under its fields, for the stage it's at.
fn stage_part(stage: Stage) -> impl IntoElement {
    let f = form();
    let out = view().gap_2().pt(px(8.0));
    match stage {
        Stage::Closed => out,
        Stage::Busy => out.child(note("Asking that machine…".to_owned())),
        Stage::Editing => out.child(small_button("computer-remote-test", "Test", test_remote)),
        Stage::Trust { fingerprint, pin, vouched } => out
            .child(view().test_id("computer-remote-fingerprint").child(note(trust_words(&fingerprint, vouched, over_https(&f.url.get_untracked())))))
            .child(small_button("computer-remote-trust", "I checked, trust it", move || {
                f.pin.set(pin.clone());
                test_remote();
            })),
        Stage::Ready { what, consent } => out.child(note(what)).child(view().test_id("computer-remote-consent").child(note(consent))).child(small_button("computer-remote-add", "I understand, add it", || add_remote(pane_signal().with_untracked(|p| p.bot.clone())))),
    }
}

/// For a test: the tab is on `bot`, with nothing behind it.
#[cfg(test)]
pub(crate) fn pane_for(bot: &str) {
    pane_signal().set(Pane { bot: bot.to_owned(), ..Pane::default() });
}

/// For a test: whether any field still holds something.
#[cfg(test)]
pub(crate) fn form_is_empty() -> bool {
    let f = form();
    [f.url, f.key_file, f.token, f.viewer, f.pin].iter().all(|s| s.with(String::is_empty)) && f.stage.with(|s| *s == Stage::Closed)
}

/// Connecting a remote computer: the address, what proves who you are, a test, then a yes.
pub(crate) fn remote_form() -> impl IntoElement {
    let f = form();
    view()
        .pt(px(10.0))
        .child(when(
            move || f.stage.with(|s| *s == Stage::Closed),
            move || {
                let setup = SETUP.with(|s| *s);
                view()
                    .flex_row()
                    .gap_2()
                    .child(small_button("computer-remote-open", "Connect a remote computer", move || f.stage.set(Stage::Editing)))
                    .child(small_button("computer-setup-open", "Set up a computer", move || setup.update(|o| *o = !*o)))
                    .into_element()
            },
        ))
        .child(when(move || f.stage.with(|s| *s == Stage::Closed) && SETUP.with(|s| s.get()), setup_part))
        .child(when(
            move || f.stage.with(|s| *s != Stage::Closed),
            move || {
                view()
                    .test_id("computer-remote-form")
                    .gap_2()
                    .child(note("A running Buwiz computer, over ssh:// or https://, or any SSH host for commands and files.".to_owned()))
                    .child(field("computer-remote-url", "ssh://you@host or https://box.example.com", false, f.url))
                    .child(when(move || !f.url.with(|u| over_https(u)), move || field("computer-remote-key", "SSH key file, like ~/.ssh/id_ed25519", false, f.key_file).into_element()))
                    .child(field("computer-remote-token", "The box's token (none for a plain SSH host)", true, f.token))
                    .child(field("computer-remote-viewer", "The box's viewer password (none for a plain SSH host)", true, f.viewer))
                    .child(view().test_id("computer-remote-note").child(text(move || f.note.get()).number_of_lines(4).text_size(px(12.0)).text_color(theme::MUTED)))
                    .child(dynamic(move || stage_part(f.stage.get()).into_element()))
                    .child(view().flex_row().child(small_button("computer-remote-cancel", "Cancel", move || f.clear())))
                    .into_element()
            },
        ))
}

/// The tab's Set up a computer: the chat's pick-a-home card, for this Bot.
fn setup_part() -> AnyElement {
    let bot = pane_signal().with_untracked(|p| p.bot.clone());
    let name = store().bots.with_untracked(|l| l.iter().find(|b| b.id == bot).map(|b| b.name.clone())).unwrap_or_default();
    view()
        .pt(px(10.0))
        .gap_2()
        .child(crate::cards::computer_setup::flow(format!("tab:{bot}"), bot, name, String::new(), crate::cards::computer_setup::Step::Pick))
        .into_element()
}

/// Opens the Computer tab on `bot` with the remote computer's form open: what picking A remote
/// computer does elsewhere.
pub(crate) fn open_remote(bot: &str) {
    let st = store();
    if pane_signal().with_untracked(|p| p.bot != bot) {
        pane_signal().set(Pane { bot: bot.to_owned(), ..Pane::default() });
        form().clear();
    }
    SETUP.with(|s| s.set(false));
    form().stage.set(Stage::Editing);
    st.detail_tab.set("computer".to_owned());
    st.right_open.set(true);
}

fn note(words: String) -> impl IntoElement {
    homes::line(words).pt(px(10.0))
}

fn small_button(id: &'static str, words: &'static str, press: impl Fn() + 'static) -> impl IntoElement {
    homes::small_button(words, words, press).test_id(id)
}

/// For a test: what a poll of `bot` would bring, and the screen's picture if there is one.
#[cfg(test)]
pub(crate) fn pane_says(bot: &str, status: Status, tile: Option<String>) {
    update(bot, |p| {
        p.status = status;
        p.tile = tile;
    });
}

/// The tab as it draws the pane, for a test with no broker to ask.
#[cfg(test)]
pub(crate) fn pane_body() -> impl IntoElement {
    let p = pane_signal().get();
    body(&p.bot, "Ann", "blob", &p).into_element()
}

/// The tab as it draws `status`, for a test with no broker to ask.
#[cfg(test)]
pub(crate) fn body_of(status: Status) -> impl IntoElement {
    let p = Pane { status, ..Pane::default() };
    body("b1", "Ann", "blob", &p).into_element()
}

/// The computer's right-click menu, as `status` allows: nothing while it comes up or before the
/// broker has said.
pub(crate) fn menu_items(bot: &str, name: &str, status: &Status) -> Vec<CtxItem> {
    match status.state.as_str() {
        "awake" => {
            let open = status.has_screen().then(|| CtxItem::row("Open", "external", "computer-open", format!("{bot}\u{1f}{name}")));
            let sleep = CtxItem::row(sleep_words(status), if status.remote.is_some() { "powerOff" } else { "moon" }, "computer-sleep", bot.to_owned());
            open.into_iter().chain([sleep]).collect()
        }
        "asleep" | "absent" | "error" => vec![CtxItem::row("Wake computer", "sun", "computer-wake", bot.to_owned())],
        _ => Vec::new(),
    }
}

fn body(bot: &str, name: &str, shape: &str, p: &Pane) -> impl IntoElement {
    let awake = p.status.state == "awake";
    let opens = awake && p.status.has_screen();
    let (b, n) = (bot.to_owned(), name.to_owned());
    let items = menu_items(bot, name, &p.status);
    let tile = view().test_id("computer-tile").h(px(180.0)).rounded(px(12.0)).bg(theme::MAIN).items_center().justify_center().overflow_hidden().on_context_menu(move |e: &ContextMenuEvent| {
        if !items.is_empty() {
            crate::ctx::open(e, items.clone());
        }
    });
    let tile = match (&p.tile, awake) {
        (Some(uri), true) => tile
            .label("Open computer")
            .child(image(uri.clone()).size_full().object_fit(ResizeMode::Contain))
            .on_press(move || open_full(&b, &n)),
        _ => {
            let words = p.status.headline();
            let tile = tile.label(if opens { "Open computer" } else { "Wake computer" }).child(sleeping_avatar(shape)).child(text(words).text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(theme::TEXT).pt(px(6.0)));
            if opens {
                // no picture yet: a press still opens it
                tile.on_press(move || open_full(&b, &n))
            } else if !awake && p.status.state != "waking" {
                tile.on_press(move || wake(b.clone()))
            } else {
                tile
            }
        }
    };
    let mut out = view().child(tile).child(text(format!("{name}'s screen")).text_size(px(12.0)).text_color(theme::MUTED).pt(px(10.0)));
    if let Some(r) = &p.status.remote {
        out = out.child(view().test_id("computer-remote-words").child(note(r.words())));
    }
    for words in [&p.status.message, &p.note].into_iter().filter(|w| !w.is_empty()) {
        out = out.child(view().test_id("computer-problem").child(note(words.clone())));
    }
    let shared = p.status.shared_with(bot);
    if !shared.is_empty() {
        out = out.child(note(shared));
    }
    if let Some(ms) = p.status.wake_ms.filter(|_| awake && woke_shows(bot)) {
        out = out.child(note(format!("Woke in {:.1} s", ms as f64 / 1000.0)));
    }
    if let Some(t) = &p.status.takeover {
        let (b1, n1, b2) = (bot.to_owned(), name.to_owned(), bot.to_owned());
        out = out.child(view().test_id("computer-takeover").child(note(format!("{name} asked you to take over: {} · {} left", t.reason, t.left())))).child(
            view()
                .flex_row()
                .gap_2()
                .pt(px(10.0))
                .child(small_button("computer-takeover-open", "Take over", move || open_full(&b1, &n1)))
                .child(small_button("computer-handback", "Hand back", move || hand_back(b2.clone()))),
        );
    } else if p.status.working {
        out = out.child(note("Its agent is using this screen.".to_owned()));
    }
    for j in &p.status.jobs {
        out = out.child(view().test_id("computer-job").child(note(j.words(p.status.remote.is_some()))));
    }
    if p.status.remote.is_some() {
        let b = bot.to_owned();
        let (words, others) = remove_words(&p.status, bot);
        if let Some(others) = others {
            out = out.child(view().test_id("computer-remote-others").child(note(others)));
        }
        out = out.child(view().flex_row().pt(px(10.0)).child(small_button("computer-remote-remove", words, move || remove_remote(b.clone()))));
    }
    if p.status.bots.len() > 1 {
        let b = bot.to_owned();
        out = out.child(view().flex_row().pt(px(10.0)).child(small_button("computer-leave", "Leave this computer", move || leave(b.clone()))));
    }
    if p.status.state == "absent" {
        if let Some(next) = &p.status.home {
            out = out.child(view().test_id("computer-next-home").child(note(format!("New computers for this Bot: {}", next.label))));
        }
        for j in &p.join {
            let (b, id) = (bot.to_owned(), j.id.clone());
            out = out.child(
                view()
                    .test_id("computer-join")
                    .label("Share a computer")
                    .pt(px(10.0))
                    .child(text(j.words()).text_size(px(12.0)).text_color(theme::TEXT))
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
async fn ticket(bot: &str, mode: &str) -> Result<String, String> {
    match client::post_any(&route(bot, "/open"), json!({"mode": mode})).await {
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
    // the mode of `url`
    let holds: Signal<String> = signal(String::new());
    let state: Signal<Status> = signal(Status::default());
    let asking = signal(false);
    // a page took the view off the gate: no more tickets until a person asks for one
    let left = signal(false);
    let driving = DRIVING.with(|s| *s);
    let look = {
        let bot = bot.clone();
        move || {
            let bot = bot.clone();
            spawn_detached(async move {
                let Some(v) = client::get_any(&route(&bot, "")).await else { return };
                let now = after_leaving(Status::parse(&v), left.get_untracked());
                let awake = now.state == "awake";
                state.set(now.clone());
                show_status(&bot, now.clone());
                // the hand-over's end decides: Take control from before it doesn't outlast it
                if now.takeover.is_some() && driving.with_untracked(|d| d.contains(&bot)) {
                    driving.update(|d| {
                        d.remove(&bot);
                    });
                }
                if !awake {
                    if !url.get_untracked().is_empty() {
                        url.set(String::new());
                        holds.set(String::new());
                    }
                } else if !asking.get_untracked() && !left.get_untracked() {
                    let wants = driving.with_untracked(|d| d.contains(&bot));
                    if let Some(mode) = next_ticket(&now, &holds.get_untracked(), wants) {
                        asking.set(true);
                        match ticket(&bot, mode).await {
                            Ok(u) => {
                                holds.set(mode.to_owned());
                                url.set(u);
                                let viewer = mode.to_owned();
                                SHOWN.with(|s| s.update(|(_, st)| st.viewer = viewer));
                            }
                            Err(message) => state.set(Status { state: "error".to_owned(), message, ..now }),
                        }
                        asking.set(false);
                    }
                }
            });
        }
    };
    set_interval(POLL, look.clone());
    // at once, and again the moment a person presses Take control
    effect(move || {
        driving.with(|_| ());
        look();
    });
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
            .on_load_start(move |page: &WebNavigation| {
                let ticket = url.get_untracked();
                if !ticket.is_empty() && !stays_on_gate(&ticket, &page.url) {
                    left.set(true);
                    url.set(String::new());
                    holds.set(String::new());
                    state.set(after_leaving(state.get_untracked(), true));
                }
            })
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
                        left.set(false);
                        if !matches!(state.with_untracked(|s| s.state.clone()).as_str(), "waking" | "awake") {
                            state.update(|s| s.state = "waking".to_owned());
                            wake(press.clone());
                        }
                    })
                    .into_element()
            },
        ))
}

/// Beside Record: what the viewer is, and the way to the other mode or to hand back.
pub(crate) fn drive_button(key: &str) -> impl IntoElement {
    let bot = bot_of_key(key).unwrap_or_default();
    let shown = SHOWN.with(|s| *s);
    let (b1, b2, b3) = (bot.clone(), bot.clone(), bot);
    let status = move |bot: &str| shown.with(|(of, s)| if of == bot { s.clone() } else { Status::default() });
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .child(view().test_id("computer-mode").child(text(move || mode_words(&status(&b1))).number_of_lines(1).text_size(px(12.0)).text_color(tc("#f5f5f5a8"))))
        .child(
            view()
                .test_id("computer-drive")
                .label("Take control")
                .h(px(32.0))
                .px(px(12.0))
                .rounded(px(16.0))
                .items_center()
                .justify_center()
                .bg(tc("#f5f5f50f"))
                .child(text(move || button_words(&status(&b2)).to_owned()).text_size(px(13.0)).text_color(tc("#f5f5f5a8")))
                .on_press(move || press_drive(&b3, &status(&b3))),
        )
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

    fn seen(state: &str, viewer: &str, takeover: Option<u64>) -> Status {
        Status { state: state.to_owned(), viewer: viewer.to_owned(), takeover: takeover.map(|s| Takeover { reason: "Sign in".to_owned(), seconds_left: s }), ..Status::default() }
    }

    #[test]
    fn the_status_says_who_has_the_screen() {
        let v = json!({"state": "awake", "viewer": "drive", "working": true, "takeover": {"reason": "Sign in", "secondsLeft": 245}});
        let s = Status::parse(&v);
        assert_eq!((s.viewer.as_str(), s.working), ("drive", true));
        assert_eq!(s.takeover, Some(Takeover { reason: "Sign in".to_owned(), seconds_left: 245 }));
        assert_eq!(s.takeover.unwrap().left(), "4:05");
        let bare = Status::parse(&json!({"state": "awake"}));
        assert_eq!((bare.viewer.as_str(), bare.working, bare.takeover), ("", false, None));
    }

    #[test]
    fn a_jobs_line_says_how_it_stands_and_no_more() {
        let job = |frozen, seconds_left| Job { name: "the build".to_owned(), bot: "Ann".to_owned(), frozen, seconds_left };
        assert_eq!(job(false, 14_340).words(false), "Ann's job \"the build\" is running · 3 h 59 min left");
        assert_eq!(job(false, 720).words(false), "Ann's job \"the build\" is running · 12 min left");
        assert_eq!(job(false, 20).words(false), "Ann's job \"the build\" is running · under a minute left");
        assert_eq!(job(true, 720).words(false), "Ann's job \"the build\" is frozen while this computer sleeps.");
        assert_eq!(job(true, 720).words(true), "Ann's job \"the build\": not connected, so how it stands isn't known.");
        assert_eq!(Job { name: String::new(), ..job(false, 720) }.words(false), "Ann's job is running · 12 min left");
    }

    #[test]
    fn sleep_says_it_freezes_a_job_and_disconnect_doesnt() {
        let job = |frozen| Job { name: "x".to_owned(), bot: "Ann".to_owned(), frozen, seconds_left: 60 };
        let on = |jobs: Vec<Job>| Status { jobs, ..seen("awake", "", None) };
        assert_eq!(sleep_words(&on(vec![])), "Sleep");
        assert_eq!(sleep_words(&on(vec![job(false)])), "Sleep, and freeze its job");
        assert_eq!(sleep_words(&on(vec![job(false), job(false)])), "Sleep, and freeze its jobs");
        let remote = Status { remote: Some(Remote::default()), ..on(vec![job(false)]) };
        assert_eq!(sleep_words(&remote), "Disconnect", "nothing there stops");
        let v = json!({"state": "awake", "jobs": [{"name": "x", "bot": "Ann", "frozen": false, "secondsLeft": 60}]});
        assert_eq!(Status::parse(&v).jobs, [job(false)]);
    }

    #[test]
    fn a_viewer_watches_until_a_person_says_and_takes_what_the_broker_says() {
        assert_eq!(next_ticket(&seen("awake", "", None), "", false), Some("watch"), "opening watches");
        assert_eq!(next_ticket(&seen("awake", "watch", None), "watch", false), None, "nothing to change");
        assert_eq!(next_ticket(&seen("awake", "watch", None), "watch", true), Some("drive"), "Take control");
        assert_eq!(next_ticket(&seen("awake", "drive", None), "drive", true), None);
        assert_eq!(next_ticket(&seen("awake", "drive", None), "drive", false), Some("watch"), "Stop driving");
        assert_eq!(next_ticket(&seen("asleep", "", None), "watch", true), None, "a sleeping computer gets no ticket");
    }

    #[test]
    fn a_hand_over_drives_and_its_end_goes_back_to_watching() {
        assert_eq!(next_ticket(&seen("awake", "watch", Some(60)), "watch", false), Some("drive"), "it's theirs");
        assert_eq!(next_ticket(&seen("awake", "drive", Some(60)), "drive", false), None);
        // the broker closed the drive session: handed back, or the time ran out
        assert_eq!(next_ticket(&seen("awake", "", None), "drive", false), Some("watch"));
        // the same session, gone under a viewer that wanted it: a new one, not a dead page
        assert_eq!(next_ticket(&seen("awake", "", None), "watch", false), Some("watch"));
    }

    #[test]
    fn the_bar_says_what_the_viewer_is_and_what_its_button_does() {
        assert_eq!(button_words(&seen("awake", "watch", None)), "Take control");
        assert_eq!(button_words(&seen("awake", "drive", None)), "Stop driving");
        assert_eq!(button_words(&seen("awake", "drive", Some(9))), "Hand back");
        assert_eq!(mode_words(&seen("awake", "watch", None)), "Watching. Your clicks don't reach the screen.");
        assert_eq!(mode_words(&Status { working: true, ..seen("awake", "watch", None) }), "Watching the agent work.");
        assert_eq!(mode_words(&seen("awake", "drive", None)), "You're driving. The agent waits while you do.");
        assert_eq!(mode_words(&seen("awake", "drive", Some(65))), "Sign in · 1:05 left");
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
        assert_eq!(joinable(&v), [Joinable { id: "c1".into(), bots: "Ann, Bea".into(), remote: false }]);
    }

    #[test]
    fn a_remote_computer_says_what_it_is_in_every_place_a_person_decides() {
        let v = json!({"state": "asleep", "remote": {"screen": false, "where": "me@h"}, "computer": "c", "bots": [{"id": "1", "name": "Ann"}]});
        let s = Status::parse(&v);
        assert_eq!(s.remote, Some(Remote { screen: false, place: "me@h".into() }));
        assert_eq!(s.headline(), "Not connected");
        assert_eq!(Status { state: "awake".into(), ..s.clone() }.headline(), "Connected");
        assert!(!s.has_screen());
        let words = s.remote.as_ref().unwrap().words();
        assert!(words.contains("whatever access you gave it") && words.contains("nothing walls it off") && words.contains("no screen"), "{words}");
        let warned = delete_warning(Some(&v), "1", "Ann").unwrap();
        assert!(warned.contains("Nothing on that machine changes") && !warned.contains("everything on it"), "{warned}");
        let row = json!({"computers": [{"id": "c1", "bots": ["Ann"], "full": false, "remote": true}]});
        assert!(joinable(&row)[0].words().contains("nothing walls it off"));
        assert!(Status::default().has_screen(), "a computer of ours has one");
    }

    #[test]
    fn a_viewer_knows_a_page_that_left_the_gate() {
        let ticket = "http://127.0.0.1:47200/s/abc/?ticket=t";
        assert!(stays_on_gate(ticket, "http://127.0.0.1:47200/s/abc/"));
        assert!(stays_on_gate(ticket, "about:blank"));
        let awake = Status { state: "awake".into(), viewer: "watch".into(), ..Status::default() };
        let shown = after_leaving(awake.clone(), true);
        assert_eq!((shown.state.as_str(), shown.viewer.as_str()), ("error", ""), "the next poll doesn't wipe it");
        assert!(shown.message.contains("somewhere else"));
        assert_eq!(after_leaving(awake.clone(), false), awake);
        for away in ["http://127.0.0.1:8787/", "http://127.0.0.1:472000/", "https://127.0.0.1:47200/", "https://evil.example/", "http://127.0.0.1:47200.evil.example/", "http://127.0.0.1:47200@evil.example/", "file:///etc/hosts", "data:text/html,x"] {
            assert!(!stays_on_gate(ticket, away), "{away}");
        }
    }

    #[test]
    fn removing_a_shared_computer_names_who_else_goes() {
        let alone = Status { bots: vec![("a".into(), "Ann".into())], ..Status::default() };
        assert_eq!(remove_words(&alone, "a"), ("Remove this computer", None));
        let shared = Status { bots: vec![("a".into(), "Ann".into()), ("b".into(), "Bea".into())], ..Status::default() };
        let (button, others) = remove_words(&shared, "a");
        assert_eq!(button, "Remove it for every Bot here");
        assert!(others.unwrap().contains("takes Bea off it too"));
    }

    #[test]
    fn a_public_authoritys_word_is_said_for_what_it_is() {
        assert!(!trust_words("ab", false, true).contains("public authority"));
        let words = trust_words("ab", true, true);
        assert!(words.contains("Nothing has been sent yet") && words.contains("public authority") && words.contains("renews"), "{words}");
    }

    #[test]
    fn a_certificate_isnt_called_a_key_on_the_machine() {
        let https = trust_words("ab", false, true);
        assert!(https.contains("the certificate on the server") && !https.contains("machine"), "{https}");
        assert!(trust_words("ab", false, false).contains("that machine's key, on the machine itself"));
    }

    #[test]
    fn https_goes_by_the_address_a_person_typed() {
        for url in ["https://box.example.com", " HTTPS://box.example.com", "https://"] {
            assert!(over_https(url), "{url}");
        }
        for url in ["", "ssh://me@h", "https", "me@h", "httpsé://x"] {
            assert!(!over_https(url), "{url}");
        }
    }

    #[test]
    fn an_edit_takes_the_form_back_from_anything_tested() {
        let trust = Stage::Trust { fingerprint: "f".into(), pin: "p".into(), vouched: false };
        let ready = Stage::Ready { what: "w".into(), consent: "c".into() };
        for stage in [trust, ready, Stage::Busy, Stage::Editing] {
            assert_eq!(stage_after_edit(&stage), Stage::Editing, "{stage:?}");
        }
        assert_eq!(stage_after_edit(&Stage::Closed), Stage::Closed);
    }

    #[test]
    fn the_form_goes_by_what_the_broker_says() {
        assert_eq!(stage_of(Some(&json!({"ok": false, "code": "trust", "fingerprint": "SHA256:x", "pin": "ssh-ed25519 A", "vouched": true}))), Ok(Stage::Trust { fingerprint: "SHA256:x".into(), pin: "ssh-ed25519 A".into(), vouched: true }));
        assert_eq!(stage_of(Some(&json!({"ok": false, "code": "remote", "message": "That machine can't be reached."}))), Err("That machine can't be reached.".to_owned()));
        assert!(stage_of(None).is_err());
        let Ok(Stage::Ready { what, consent }) = stage_of(Some(&json!({"ok": true, "screen": false, "system": "Linux", "consent": "words"}))) else { panic!() };
        assert!(what.contains("Linux") && what.contains("no screen"), "{what}");
        assert_eq!(consent, "words");
        let Ok(Stage::Ready { what, .. }) = stage_of(Some(&json!({"ok": true, "screen": true, "consent": "w"}))) else { panic!() };
        assert!(what.contains("haven't been sent") && what.contains("when you say yes"), "{what}");
    }
}
