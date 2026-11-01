//! The Details pane's Computer tab: the bot's computer (a box the
//! broker provisions) — its status, Start, Update, Reset with a confirm,
//! the network-policy choice and a screen tile refreshed every 2 s while
//! the tab is open. The pane NativeChat has (`opengrok/src/components/
//! computer.rs`, `opengrok/src/state.rs`), over opengrok-server's routes
//! with a Buwiz Bot `Bot.id` in the `{coworker_id}` slot:
//!
//! - `GET`/`POST /coworkers/{id}/computer` — status / wake
//! - `POST /coworkers/{id}/computer/update`, `POST …/computer/reset`
//! - `GET`/`PUT /coworkers/{id}/computer/egress-policy` `{"mode": …}`
//! - `GET /coworkers/{id}/screen` — the latest screenshot
//!
//! Until the broker ports them (S5) they answer `501 not_ported`: the
//! pane then says, quietly, that this broker has no computer yet.

use nsh_core::prelude::*;
use serde_json::Value;

use crate::client;
use crate::live::{keep, store};
use crate::theme;

// ————————————————————————— wire —————————————————————————

/// `GET /coworkers/{id}/computer` — opengrok-server
/// `agui/provision.rs` `coworker_screen` (also what `POST …/computer`,
/// `…/computer/update` answer). Read fields are copied out; `raw` keeps
/// every field the server sent, unknown ones included.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Computer {
    /// `state`: `absent`, `starting`, `running`, `stopped`, `error`, …
    pub state: String,
    /// `boxId`: present once the bot has a box.
    pub box_id: Option<String>,
    /// `image`: `{running, latest, stale}` — whether a newer image exists.
    pub image: Option<ImageState>,
    /// `update`: `{phase, error}` while an update runs or after it failed.
    pub update: Option<UpdateState>,
    /// `computerError`: `{code, message}` — why there is no box (Docker
    /// missing, …), shown as a sentence.
    pub error: Option<ComputerError>,
    /// `egressPolicy`: `bypass` | `ask` | `never`.
    pub egress: Option<Egress>,
    /// `screenStatus` / `screenStatusText`: the screen's readiness.
    pub screen_status: Option<String>,
    pub screen_status_text: Option<String>,
    pub raw: Value,
}

/// `image` of the computer status (`agui/provision.rs`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ImageState {
    pub running: Option<String>,
    pub latest: Option<String>,
    pub stale: bool,
}

/// `update` of the computer status (`agui/provision.rs`): `phase` is
/// `pulling`, `transferring`, `starting` or `failed`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct UpdateState {
    pub phase: String,
    pub error: Option<String>,
}

/// `computerError` of the computer status, and the body of an error
/// status from `POST …/computer` (`agui/routes/computer.rs`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ComputerError {
    pub code: String,
    pub message: String,
}

/// The network policy, `opengrok/src/opengrok/client.rs` `LocalExecMode`:
/// the wire keeps `bypass` for Always (`as_stored`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Egress {
    Always,
    Ask,
    Never,
}

impl Egress {
    pub const ALL: [Egress; 3] = [Egress::Always, Egress::Ask, Egress::Never];

    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "bypass" | "always" => Some(Self::Always),
            "ask" => Some(Self::Ask),
            "never" => Some(Self::Never),
            _ => None,
        }
    }

    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Always => "bypass",
            Self::Ask => "ask",
            Self::Never => "never",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Always => "Always",
            Self::Ask => "Ask",
            Self::Never => "Never",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Always => "computer-network-always",
            Self::Ask => "computer-network-ask",
            Self::Never => "computer-network-never",
        }
    }
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

impl Computer {
    pub fn parse(v: &Value) -> Self {
        let image = v.get("image").filter(|i| i.is_object()).map(|i| ImageState {
            running: str_of(i, "running"),
            latest: str_of(i, "latest"),
            stale: i.get("stale").and_then(Value::as_bool).unwrap_or(false),
        });
        let update = v.get("update").filter(|u| u.is_object()).map(|u| UpdateState {
            phase: str_of(u, "phase").unwrap_or_default(),
            error: str_of(u, "error"),
        });
        Self {
            state: str_of(v, "state").unwrap_or_else(|| "absent".to_owned()),
            box_id: str_of(v, "boxId").filter(|b| !b.is_empty()),
            image,
            update,
            error: v.get("computerError").and_then(ComputerError::parse),
            egress: str_of(v, "egressPolicy").as_deref().and_then(Egress::from_wire),
            screen_status: str_of(v, "screenStatus"),
            screen_status_text: str_of(v, "screenStatusText"),
            raw: v.clone(),
        }
    }

    /// Whether the bot has a box (`boxId`, or a state that implies one).
    pub fn present(&self) -> bool {
        self.box_id.is_some() || matches!(self.state.as_str(), "starting" | "running" | "stopped")
    }

    pub fn running(&self) -> bool {
        self.state == "running"
    }

    /// An update is under way (`opengrok/src/opengrok/client.rs`
    /// `CoworkerComputer::updating`): any phase but `failed`.
    pub fn updating(&self) -> bool {
        self.update.as_ref().is_some_and(|u| u.phase != "failed")
    }

    /// A newer image exists (`CoworkerComputer::image_stale`).
    pub fn image_stale(&self) -> bool {
        self.image.as_ref().is_some_and(|i| i.stale || (i.running.is_some() && i.latest.is_some() && i.running != i.latest))
    }

    /// The image is known and the newest.
    pub fn image_current(&self) -> bool {
        self.image.as_ref().is_some_and(|i| i.running.is_some() && !self.image_stale())
    }

    /// The status line (`opengrok/src/components/computer.rs`
    /// `ComputerStatus`): the update's step while one runs, else the state.
    pub fn status_words(&self) -> String {
        if let Some(u) = self.update.as_ref().filter(|u| u.phase != "failed") {
            return match u.phase.as_str() {
                "pulling" => "Fetching the newest image…",
                "transferring" => "Transferring your data…",
                _ => "Starting the new computer…",
            }
            .to_owned();
        }
        match self.state.as_str() {
            "absent" | "" => "No computer yet",
            "starting" | "creating" | "provisioning" | "pending" => "Starting…",
            "running" => "Running",
            "stopped" | "exited" => "Stopped",
            "error" | "failed" => "Error",
            other => return sentence(other),
        }
        .to_owned()
    }

    /// A failed update's sentence (`UpdateStatus::detail`).
    pub fn update_failure(&self) -> Option<String> {
        let u = self.update.as_ref().filter(|u| u.phase == "failed")?;
        Some(sentence(u.error.as_deref().unwrap_or("The update failed")))
    }
}

impl ComputerError {
    /// `{code, message}`; a bare `{error}` string counts too (a 503's body).
    pub fn parse(v: &Value) -> Option<Self> {
        if !v.is_object() {
            return None;
        }
        let mut message = str_of(v, "message").or_else(|| str_of(v, "error"))?;
        let mut code = str_of(v, "code").unwrap_or_default();
        // "docker_unavailable: docker is not running": the code rides in front
        if let Some((head, rest)) = message.split_once(": ")
            && !head.is_empty()
            && head.chars().all(|c| c.is_ascii_lowercase() || c == '_')
        {
            if code.is_empty() {
                code = head.to_owned();
            }
            message = rest.to_owned();
        }
        if message.trim().is_empty() {
            return None;
        }
        Some(Self { code, message })
    }
}

/// `s` capitalised and ending in a period (`computer.rs` `sentence`).
pub(crate) fn sentence(s: &str) -> String {
    let s = s.trim();
    let mut out: String = s.chars().take(1).flat_map(char::to_uppercase).collect();
    out.push_str(s.get(s.chars().next().map_or(0, char::len_utf8)..).unwrap_or_default());
    if !out.is_empty() && !out.ends_with(['.', '!', '?', '…']) {
        out.push('.');
    }
    out
}

/// The Update button's rest label (`computer.rs` `update_rest_label`).
pub(crate) fn update_rest_label(stale: bool, current: bool) -> &'static str {
    if stale {
        "Update available"
    } else if current {
        "Up to date"
    } else {
        "Update"
    }
}

/// `true` when a reply is the broker's `501 not_ported`
/// (`broker/rox-broker/src/hub/types.rs` `not_ported`).
pub(crate) fn is_not_ported(v: &Value) -> bool {
    v.get("code").and_then(Value::as_str) == Some("not_ported")
}

// ————————————————————————— state —————————————————————————

/// What the broker said about the routes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Route {
    /// Not asked yet, or the broker was unreachable.
    #[default]
    Unknown,
    /// `501 not_ported`: this broker has no computer routes yet.
    Missing,
    Live,
}

/// An action awaiting its confirm, or running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Update,
    Reset,
}

impl Action {
    fn verb(self) -> &'static str {
        match self {
            Self::Update => "Update",
            Self::Reset => "Reset",
        }
    }

    fn question(self) -> &'static str {
        match self {
            Self::Update => "Update this computer? It restarts on the newest image; your files stay.",
            Self::Reset => "Reset this computer? Everything on it is lost.",
        }
    }

    fn busy(self) -> &'static str {
        match self {
            Self::Update => "Updating…",
            Self::Reset => "Resetting…",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Update => "update",
            Self::Reset => "reset",
        }
    }
}

/// The Computer tab's model for one bot — kept across live patches.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Pane {
    /// The bot the rest is about: another bot starts over.
    pub bot: String,
    pub route: Route,
    pub computer: Option<Computer>,
    /// The latest screen as a `data:` URI (the bytes compared before a
    /// swap, so an unchanged screen never repaints).
    pub screen: Option<String>,
    pub screen_bytes: Vec<u8>,
    /// Start is on its way.
    pub asking: bool,
    /// Wake/Start was tried once for an absent box (the heal
    /// `opengrok/src/state.rs` `refresh_coworker_computer` does).
    pub healed: bool,
    pub confirm: Option<Action>,
    pub acting: Option<Action>,
    /// A refused Start/Update/Reset, as a sentence.
    pub action_error: Option<String>,
    /// A policy write on its way (shown at once; the poll catches up).
    pub pending_egress: Option<Egress>,
}

thread_local! {
    // `keep` restores once per live patch, so the signal is made once.
    static PANE: Signal<Pane> = keep("computer.pane", Pane::default);
}

fn pane_signal() -> Signal<Pane> {
    PANE.with(|s| s.clone())
}

/// The pane's state for `bot`, starting over when the bot changed.
fn for_bot(bot: &str) -> Pane {
    let sig = pane_signal();
    let cur = sig.get_untracked();
    if cur.bot == bot {
        return cur;
    }
    let fresh = Pane { bot: bot.to_owned(), ..Default::default() };
    sig.set(fresh.clone());
    fresh
}

fn update_pane(bot: &str, f: impl FnOnce(&mut Pane)) {
    let sig = pane_signal();
    sig.update(|p| {
        if p.bot == bot {
            f(p);
        }
    });
}

fn path(bot: &str, rest: &str) -> String {
    format!("/coworkers/{}/computer{rest}", client::urlenc(bot))
}

/// `GET /coworkers/{id}/computer` into the pane; a 501 marks the routes
/// missing. An absent box is woken once (`refresh_coworker_computer`).
async fn refresh_status(bot: String) {
    let Some((status, reply)) = client::get_any(&path(&bot, "")).await else { return };
    // 501 `not_ported`, or a hub that does not know the path at all
    if is_not_ported(&reply) || (status == 404 && reply.get("state").is_none()) || status == 501 {
        update_pane(&bot, |p| p.route = Route::Missing);
        return;
    }
    if !(200..300).contains(&status) {
        return;
    }
    let c = Computer::parse(&reply);
    let heal = c.state == "absent" && c.error.is_none() && !c.updating();
    let mut start = false;
    update_pane(&bot, |p| {
        p.route = Route::Live;
        if p.acting.is_some() && c.updating() {
            // the server shows the update's own progress now
            p.acting = None;
        }
        if let Some(e) = c.egress
            && p.pending_egress == Some(e)
        {
            p.pending_egress = None;
        }
        if !c.running() {
            p.screen = None;
            p.screen_bytes.clear();
        }
        p.computer = Some(c.clone());
        if heal && !p.healed && !p.asking {
            p.healed = true;
            start = true;
        }
    });
    if start {
        start_computer(bot).await;
    }
}

/// `GET /coworkers/{id}/screen` while the box runs: a PNG, or (an older
/// server, `agui/routes/computer.rs` `computer_screen`) JSON with the PNG
/// in `base64`.
async fn refresh_screen(bot: String) {
    let running = pane_signal().with_untracked(|p| p.bot == bot && p.computer.as_ref().is_some_and(Computer::running));
    if !running {
        return;
    }
    let path = format!("/coworkers/{}/screen", client::urlenc(&bot));
    let Some((mime, body)) = client::get_bytes(&path).await else { return };
    let (mime, bytes) = if mime.starts_with("application/json") || body.first() == Some(&b'{') {
        let Ok(v) = serde_json::from_slice::<Value>(&body) else { return };
        let Some(b64) = v.get("base64").and_then(Value::as_str) else { return };
        let Ok((_, bytes)) = nsh_core::image::decode_data_uri(&format!("data:;base64,{b64}")) else { return };
        (str_of(&v, "mime").unwrap_or_else(|| "image/png".to_owned()), bytes)
    } else {
        (if mime.is_empty() { "image/png".to_owned() } else { mime }, body)
    };
    if bytes.is_empty() {
        return;
    }
    update_pane(&bot, |p| {
        if p.screen_bytes != bytes {
            p.screen = Some(nsh_core::platform::clipboard::data_uri(mime.split(';').next().unwrap_or("image/png"), &bytes));
            p.screen_bytes = bytes;
        }
    });
}

/// `POST /coworkers/{id}/computer` (`ensure_coworker_computer`): wakes or
/// makes the box; a refusal (Docker missing, …) is kept as a sentence.
async fn start_computer(bot: String) {
    update_pane(&bot, |p| {
        p.asking = true;
        p.action_error = None;
    });
    let reply = client::post_any(&path(&bot, ""), serde_json::json!({})).await;
    update_pane(&bot, |p| {
        p.asking = false;
        match reply.as_ref() {
            Some(v) if is_not_ported(v) => p.route = Route::Missing,
            Some(v) if v.get("state").is_some() => p.computer = Some(Computer::parse(v)),
            Some(v) => p.action_error = ComputerError::parse(v).map(|e| sentence(&e.message)),
            None => p.action_error = Some("The broker did not answer.".to_owned()),
        }
    });
}

/// `POST …/computer/update` or `…/computer/reset` (`start_computer_update`,
/// `start_computer_reset`): the reply is the new status.
async fn run_action(bot: String, action: Action) {
    update_pane(&bot, |p| {
        p.acting = Some(action);
        p.confirm = None;
        p.action_error = None;
    });
    let reply = client::post_any(&path(&bot, &format!("/{}", action.path())), serde_json::json!({})).await;
    update_pane(&bot, |p| {
        match reply.as_ref() {
            Some(v) if is_not_ported(v) => {
                p.route = Route::Missing;
                p.acting = None;
            }
            Some(v) if v.get("state").is_some() => {
                let c = Computer::parse(v);
                if !c.updating() {
                    p.acting = None;
                }
                p.computer = Some(c);
                p.screen = None;
                p.screen_bytes.clear();
            }
            Some(v) => {
                p.acting = None;
                p.action_error = ComputerError::parse(v).map(|e| sentence(&e.message)).or_else(|| Some("The server refused; nothing was changed.".to_owned()));
            }
            None => {
                p.acting = None;
                p.action_error = Some("The broker did not answer.".to_owned());
            }
        }
    });
}

/// `PUT …/computer/egress-policy {"mode"}` (`set_egress_policy`): shown at
/// once, confirmed by the next status.
async fn set_egress(bot: String, mode: Egress) {
    update_pane(&bot, |p| {
        p.pending_egress = Some(mode);
        p.action_error = None;
    });
    let reply = client::put(&path(&bot, "/egress-policy"), serde_json::json!({ "mode": mode.as_wire() })).await;
    update_pane(&bot, |p| match reply.as_ref() {
        Some(Value::Null) | Some(Value::Object(_)) if reply.as_ref().is_some_and(|v| ComputerError::parse(v).is_none()) => {
            if let Some(c) = p.computer.as_mut() {
                c.egress = Some(mode);
            }
            p.pending_egress = None;
        }
        Some(v) => {
            p.pending_egress = None;
            p.action_error = ComputerError::parse(v).map(|e| sentence(&e.message));
        }
        None => {
            p.pending_egress = None;
            p.action_error = Some("The broker did not answer.".to_owned());
        }
    });
    refresh_status(bot).await;
}

/// The 2 s poll while the tab is open (`opengrok/src/state.rs`
/// `COMPUTER_POLL`): status, then the screen while the box runs.
const POLL: std::time::Duration = std::time::Duration::from_secs(2);

fn refresh(bot: String) {
    spawn(async move {
        refresh_status(bot.clone()).await;
        refresh_screen(bot).await;
    });
}

// ————————————————————————— pane —————————————————————————

/// The Computer tab's body. Built when the tab opens; its poll runs
/// while it is shown and stops with it.
pub(crate) fn pane() -> impl IntoElement {
    if let Some(b) = store().active_bot() {
        for_bot(&b.id);
        refresh(b.id);
    }
    set_interval(POLL, || {
        if store().detail_tab.get_untracked() != "computer" {
            return;
        }
        if let Some(b) = store().active_bot() {
            for_bot(&b.id);
            refresh(b.id);
        }
    });
    view().test_id("computer-pane").child(dynamic(|| {
        let Some(b) = store().active_bot() else {
            return view().into_element();
        };
        let p = pane_signal().get();
        if p.bot != b.id {
            return view().into_element();
        }
        body(&b.id, &b.name, &b.shape, &p).into_element()
    }))
}

fn note(id: &str, t: String, ink: AdaptiveColor) -> AnyElement {
    text(t).test_id(id).text_size(px(12.0)).line_height(px(16.0)).text_color(ink).into_element()
}

fn body(bot: &str, name: &str, shape: &str, p: &Pane) -> impl IntoElement {
    let card = view().bg(theme::CARD2).rounded(px(12.0)).border_1().border_color(theme::BORDER_F).p(px(12.0)).gap(px(10.0));
    match p.route {
        // the broker's 501: a quiet line, no controls
        Route::Missing => {
            return card
                .child(note("computer-missing", "This broker has no computer yet".to_owned(), theme::DIM))
                .into_element();
        }
        Route::Unknown => {
            return card.child(note("computer-status", "Looking for a computer…".to_owned(), theme::DIM)).into_element();
        }
        Route::Live => {}
    }
    let c = p.computer.clone().unwrap_or_default();
    let present = c.present();
    let busy = p.acting.is_some() || c.updating();
    let status = match p.acting {
        Some(a) if !c.updating() => a.busy().to_owned(),
        _ if p.asking => "Starting…".to_owned(),
        _ => c.status_words(),
    };
    let error = p
        .action_error
        .clone()
        .or_else(|| c.error.as_ref().map(|e| sentence(&e.message)))
        .or_else(|| c.update_failure());
    let bot_id = bot.to_owned();
    card
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(
                    text(status)
                        .test_id("computer-status")
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::TEXT)
                        .flex_1(),
                )
                .when(c.box_id.is_some(), |v| {
                    v.child(crate::cards::pill(c.box_id.as_deref().unwrap_or_default()))
                }),
        )
        .child(screen_tile(name, shape, &c, p))
        .when(error.is_some(), |v| v.child(note("computer-error", error.clone().unwrap_or_default(), theme::DOWN)))
        .child(controls(bot_id.clone(), &c, p, present, busy))
        .when(p.confirm.is_some(), |v| v.child(confirm_row(bot_id.clone(), p.confirm.unwrap_or(Action::Update))))
        .when(present, |v| v.child(network_row(bot_id, &c, p)))
        .into_element()
}

/// The screen: the latest PNG while the box runs, else the sleeping bot
/// (no box yet) or a dark tile with the screen's own status.
fn screen_tile(name: &str, shape: &str, c: &Computer, p: &Pane) -> impl IntoElement {
    let tile = view()
        .test_id("computer-screen")
        .h(px(180.0))
        .rounded(px(10.0))
        .bg(theme::MAIN)
        .border_1()
        .border_color(theme::BORDER_F)
        .overflow_hidden()
        .items_center()
        .justify_center();
    let tile = match (&p.screen, c.running(), c.present()) {
        (Some(uri), true, _) => tile.child(image(uri.clone()).object_fit(ResizeMode::Contain).w_full().h_full()),
        (None, true, _) => tile
            .child(crate::icons::icon_ink("computer", 28.0, || theme::FAINT))
            .child(note("computer-screen-note", c.screen_status_text.clone().unwrap_or_else(|| "Waiting for the screen…".to_owned()), theme::MUTED)),
        (_, false, true) => tile
            .child(crate::icons::icon_ink("computer", 28.0, || theme::FAINT))
            .child(note("computer-screen-note", c.status_words(), theme::MUTED)),
        (_, false, false) => tile
            .child(crate::avatar::sleeping_blob(shape, 33.0, "#f5f5f58a"))
            .child(note("computer-screen-note", "No computer yet".to_owned(), theme::MUTED)),
    };
    view()
        .gap(px(6.0))
        .child(tile)
        .child(text(format!("{name}'s screen")).text_size(px(12.0)).text_color(theme::MUTED))
}

/// Start, Update (Up to date / Update available) and Reset.
fn controls(bot: String, c: &Computer, p: &Pane, present: bool, busy: bool) -> impl IntoElement {
    let stale = c.image_stale();
    let current = c.image_current();
    let update_label = match p.acting {
        Some(Action::Update) => "Updating…",
        _ if c.updating() => "Updating…",
        _ => update_rest_label(stale, current),
    };
    let reset_label = if p.acting == Some(Action::Reset) { "Resetting…" } else { "Reset this computer" };
    let can_start = !p.asking && !busy && (!present || c.state == "stopped");
    let start_label = if p.asking { "Starting…" } else if present { "Start" } else { "Get a computer" };
    let b1 = bot;
    view()
        .flex_row()
        .flex_wrap()
        .gap(px(8.0))
        .test_id("computer-controls")
        .child(ctl("computer-start", start_label, true, can_start, move || {
            let bot = b1.clone();
            crate::store::spawn_detached(async move { start_computer(bot).await });
        }))
        .child(ctl("computer-update", update_label, stale, present && !busy && !(current && !stale), || {
            pane_signal().update(|p| p.confirm = Some(Action::Update));
        }))
        .child(ctl("computer-reset", reset_label, false, present && !busy, || {
            pane_signal().update(|p| p.confirm = Some(Action::Reset));
        }))
}

/// "Update this computer?" / "Reset this computer?" with its verb and Cancel.
fn confirm_row(bot: String, action: Action) -> impl IntoElement {
    view()
        .test_id("computer-confirm")
        .rounded(px(10.0))
        .bg(theme::SUBTLE)
        .p(px(10.0))
        .gap(px(8.0))
        .child(text(action.question()).text_size(px(12.5)).line_height(px(17.0)).text_color(theme::TEXT))
        .child(
            view()
                .flex_row()
                .gap(px(8.0))
                .child(ctl("computer-confirm-yes", action.verb(), true, true, move || {
                    let bot = bot.clone();
                    crate::store::spawn_detached(async move { run_action(bot, action).await });
                }))
                .child(ctl("computer-confirm-cancel", "Cancel", false, true, || {
                    pane_signal().update(|p| p.confirm = None);
                })),
        )
}

/// "Use your network from this computer": Always / Ask / Never.
fn network_row(bot: String, c: &Computer, p: &Pane) -> impl IntoElement {
    let chosen = p.pending_egress.or(c.egress);
    view()
        .test_id("computer-network")
        .gap(px(6.0))
        .child(text("Use your network from this computer").text_size(px(12.0)).text_color(theme::DIM))
        .child(view().flex_row().gap(px(6.0)).children(Egress::ALL.into_iter().map(|mode| {
            let bot = bot.clone();
            let on = chosen == Some(mode);
            seg(mode.id(), mode.label(), on, move || {
                if on {
                    return;
                }
                let bot = bot.clone();
                crate::store::spawn_detached(async move { set_egress(bot, mode).await });
            })
        })))
        .when(chosen.is_none(), |v| {
            v.child(note("computer-network-note", "This computer has no network rule from the server yet.".to_owned(), theme::MUTED))
        })
}

/// A card button with a stable id (`cards::btn`, plus disabled).
fn ctl(id: &str, label: &str, primary: bool, enabled: bool, on: impl Fn() + 'static) -> impl IntoElement {
    let hov = signal(false);
    let (bg, bg_hov, ink) = if primary {
        (theme::TEXT, theme::DIM, theme::MAIN)
    } else {
        (theme::HOVER, theme::ACTIVE, theme::TEXT)
    };
    view()
        .test_id(id)
        .label(label.to_owned())
        .h(px(28.0))
        .px(px(12.0))
        .rounded(px(14.0))
        .items_center()
        .justify_center()
        .opacity(if enabled { 1.0 } else { 0.45 })
        .bg(move || if hov.get() && enabled { bg_hov } else { bg })
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(move || {
            if enabled {
                on()
            }
        })
        .child(text(label.to_owned()).number_of_lines(1).text_size(px(12.5)).line_height(px(16.0)).text_color(ink))
}

/// One choice of a segmented control.
fn seg(id: &str, label: &str, on: bool, press: impl Fn() + 'static) -> impl IntoElement {
    let hov = signal(false);
    view()
        .test_id(id)
        .label(label.to_owned())
        .h(px(26.0))
        .px(px(12.0))
        .rounded(px(13.0))
        .items_center()
        .justify_center()
        .border_1()
        .border_color(if on { theme::TEXT } else { theme::BORDER_S })
        .bg(move || if on { theme::TEXT } else if hov.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(move || press())
        .child(text(label.to_owned()).text_size(px(12.0)).text_color(if on { theme::MAIN } else { theme::TEXT }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_status_and_keeps_unknown_fields() {
        let v = serde_json::json!({
            "agentId": "b1", "state": "running", "boxId": "box-7",
            "image": {"running": "v1", "latest": "v2", "stale": true},
            "egressPolicy": "bypass", "shareScope": "team", "extra": {"k": 1}
        });
        let c = Computer::parse(&v);
        assert!(c.running() && c.present() && c.image_stale() && !c.image_current());
        assert_eq!(c.box_id.as_deref(), Some("box-7"));
        assert_eq!(c.egress, Some(Egress::Always));
        assert_eq!(c.raw["extra"]["k"], 1);
        assert_eq!(c.status_words(), "Running");
    }

    #[test]
    fn an_update_speaks_before_the_state() {
        let c = Computer::parse(&serde_json::json!({"state": "running", "update": {"phase": "pulling"}}));
        assert!(c.updating());
        assert_eq!(c.status_words(), "Fetching the newest image…");
        let failed = Computer::parse(&serde_json::json!({"state": "running", "update": {"phase": "failed", "error": "no disk left"}}));
        assert!(!failed.updating());
        assert_eq!(failed.update_failure().as_deref(), Some("No disk left."));
    }

    #[test]
    fn errors_become_sentences_and_501_is_not_ported() {
        let e = ComputerError::parse(&serde_json::json!({"code": "docker_missing", "message": "docker is not installed"}));
        assert_eq!(e.map(|e| sentence(&e.message)).as_deref(), Some("Docker is not installed."));
        let e = ComputerError::parse(&serde_json::json!({"error": "docker_unavailable: docker is not running"}));
        assert_eq!(e.as_ref().map(|e| e.code.as_str()), Some("docker_unavailable"));
        assert_eq!(e.map(|e| sentence(&e.message)).as_deref(), Some("Docker is not running."));
        assert!(is_not_ported(&serde_json::json!({"ok": false, "code": "not_ported"})));
        assert!(!is_not_ported(&serde_json::json!({"state": "absent"})));
        assert_eq!(Egress::from_wire("bypass"), Some(Egress::Always));
        assert_eq!(Egress::Never.as_wire(), "never");
        assert_eq!(update_rest_label(true, true), "Update available");
        assert_eq!(update_rest_label(false, true), "Up to date");
        assert_eq!(update_rest_label(false, false), "Update");
    }
}
