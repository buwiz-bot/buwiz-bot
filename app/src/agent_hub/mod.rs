//! The Agent Hub: how the broker's model sees mini app screens.
//!
//! The Hub lives in the app and reads nsh's tree in the process, so a
//! shipped Bot opens no listener. The broker reaches it over the connection
//! the Bot makes itself: its requests come down `GET /v1/host/ui/stream`,
//! the answers go up `POST /v1/host/ui/reply`, both with the host token.
//! `/v1/host/events` isn't used: it is a broadcast and drops on lag.
//!
//! Each message is a `buwiz-agent` [`Envelope`] or [`Response`]. Two kinds
//! are answered. `snapshot` to the target `bot`, the Hub itself: its
//! answer's `result` is `{"apps": [UiTree, …]}`, a tree for each app with
//! something open, each screen a `page` node. `click`, `type` and
//! `set_value` to a mini app's target, on the node of a Hub id: the same
//! answer after the action, with `refused`, what the action's calls were
//! refused, as sentences. Anything else is refused ([`press`]). A message's
//! `caller` is never read.
//!
//! An action reaches only the screen on top of the full-screen stack, and
//! only as the model last saw it: [`Looked`] is that screen's number and
//! [`filter::rev`], and the action reads the tree again first. A changed
//! screen, a covered one or a stale id refuses, and a node with no name
//! is never pressed: a name is what says what a press does, and what's left
//! unnamed may be a row of data. The way from a Hub id to nsh's is made
//! from that same read, in the Bot, so all that's taken from the model is
//! the id. The screen is read again in the step that presses, in nsh, so
//! what moved while nsh found the node gets no press. Only an action that
//! went through, and whose answer carries the screen, moves [`Looked`] on:
//! after any other the model saw nothing new.
//!
//! The input goes through [`AppView::act`], which tells
//! [`crate::mini_v1::attribution`] the agent is acting, so what it starts
//! goes out as the agent's in any build. What the press does is decided
//! where the call is made, not here: a button doesn't say what it runs, its
//! tool does ([`press::check_tool`], in the bridge).
//!
//! A look shows, and an action reaches, a screen only for the Bot whose chat
//! opened it: an open keeps its Bot ([`Open::bot`]), the message says whose
//! turn it is, and they have to match ([`mine`]). That word only narrows, so
//! the broker saying it costs nothing: it could already refuse.
//!
//! A call that needs the person waits in the bridge ([`hold`]): its card goes
//! up over the top screen ([`card`]) and comes down when the person answers,
//! or when that screen changes, covers or closes ([`show_card`]).
//!
//! Nothing reads the Bot's own windows: only opens that [`opened`] named,
//! and [`filter`] runs on each here, before anything leaves the app. The
//! model's tools, and the per-Bot switches that leave them off, are the
//! broker's (`hub/tools/ui.rs`).

pub mod card;
pub mod filter;
pub mod hold;
pub mod press;

use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

use buwiz_agent::{AgentId, Envelope, Op, PlatformKind, Response, Target, UiTree};
use buwiz_protocol::ui;
use nsh_platform_macos::{AppView, SCREEN_CHANGED, UiAct, ui_tree::UiNode as Seen};
use serde_json::json;

use crate::mini_v1::{
    attribution,
    host_api::{Broker, HttpBroker},
    native::on_main,
    to_store,
};
use buwiz_agent::OnceId;
use filter::Reading;
use hold::{Answer, Card};
use press::Held;

const STREAM: &str = "/v1/host/ui/stream";
const REPLY: &str = "/v1/host/ui/reply";

/// How long what an action started has to show: its calls have to be over
/// for this long.
const QUIET: Duration = Duration::from_millis(250);
const CALLS_WAIT: Duration = ui::CALLS_WAIT;
const ACT_LIMIT: Duration = ui::ACT_LIMIT;

/// A running open.
struct Open {
    n: u64,
    /// Its module key: its place on the full-screen stack. Holds its ticket,
    /// so it never leaves the app.
    key: String,
    target: String,
    /// What the screen is called, from the manifest: an opener's title can
    /// be a taxpayer's name.
    name: String,
    /// The app in the manifest.
    app: String,
    /// The Bot whose chat it was opened from: `Some(None)` is a plain chat,
    /// and `None` not known yet.
    bot: Option<Option<String>>,
    view: Weak<AppView>,
}

/// The top screen as the model last saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Looked {
    n: u64,
    rev: u64,
}

thread_local! {
    static OPENS: RefCell<Vec<Open>> = const { RefCell::new(Vec::new()) };
    static LOOKED: Cell<Option<Looked>> = const { Cell::new(None) };
    /// How many looks there were: what an action started with.
    static LOOKS: Cell<u64> = const { Cell::new(0) };
    /// The action running: one at a time.
    static ACTING: Cell<Option<Acting>> = const { Cell::new(None) };
    static ACTIONS: Cell<u64> = const { Cell::new(0) };
}

/// An action in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Acting {
    n: u64,
    since: Instant,
}

/// Whether `acting` still holds the Hub at `now`: one whose end went
/// missing lets go.
fn busy(acting: Option<Acting>, now: Instant) -> bool {
    acting.is_some_and(|a| now.saturating_duration_since(a.since) < ACT_LIMIT)
}

/// The last look after an action: the screen it answered with, if it went
/// through, is still the one in charge, and no look came in meanwhile.
fn look_after(ok: bool, current: bool, looked_meanwhile: bool, last: Option<Looked>, now: Option<Looked>) -> Option<Looked> {
    if ok && current && !looked_meanwhile { now } else { last }
}

/// An open started, on the main thread. `short` is its plugin's.
pub fn opened(n: u64, key: &str, short: &str, plugin_title: &str, app: &str, title: &str, view: Weak<AppView>) {
    let name = if title.is_empty() || title == plugin_title { plugin_title.to_owned() } else { format!("{plugin_title}: {title}") };
    OPENS.with_borrow_mut(|o| o.push(Open { n, key: key.to_owned(), target: short.to_owned(), name, app: app.to_owned(), bot: None, view }));
    // ahead of any action: both go through the store, then the main thread
    to_store(move |st| {
        let bot = st.active_bot_untracked().map(|b| b.id);
        on_main(move || OPENS.with_borrow_mut(|o| o.iter_mut().filter(|open| open.n == n).for_each(|open| open.bot = Some(bot.clone()))));
    });
}

/// Open `n` ended, on the main thread.
pub fn closed(n: u64) {
    OPENS.with_borrow_mut(|o| o.retain(|open| open.n != n));
}

/// One open as it is now.
struct Surface {
    n: u64,
    view: Weak<AppView>,
    key: String,
    target: String,
    name: String,
    app: String,
    bot: Option<Option<String>>,
    nodes: Vec<Seen>,
}

/// The opens with a window, oldest first.
fn surfaces() -> Vec<Surface> {
    OPENS.with_borrow(|opens| {
        opens
            .iter()
            .filter_map(|o| {
                let view = o.view.upgrade()?;
                let shown = view.view();
                (shown.window().is_some() && !shown.isHiddenOrHasHiddenAncestor()).then(|| Surface {
                    n: o.n,
                    view: o.view.clone(),
                    key: o.key.clone(),
                    target: o.target.clone(),
                    name: o.name.clone(),
                    app: o.app.clone(),
                    bot: o.bot.clone(),
                    nodes: view.ui_tree().nodes,
                })
            })
            .collect()
    })
}

/// What `surfaces` come to: a tree per app, in order of first open. `stack`
/// is the full-screen stack's keys, bottom first: what stays under its top
/// one is `covered`, an open off the stack a `card` in the chat. A surface
/// with no valid target or id stays out.
fn trees(surfaces: &[Surface], stack: &[String], reading: Reading) -> Vec<UiTree> {
    let top = top_of(surfaces, stack);
    let mut out: Vec<UiTree> = Vec::new();
    for (i, s) in surfaces.iter().enumerate() {
        let Ok(target) = Target::parse(&s.target) else { continue };
        let at = out.iter().position(|t| t.target == target).unwrap_or_else(|| {
            out.push(UiTree { target, platform: PlatformKind::Desktop, ready: true, nodes: Vec::new(), rev: None });
            out.len() - 1
        });
        let state = if Some(i) == top {
            "top"
        } else if stack.contains(&s.key) {
            "covered"
        } else {
            "card"
        };
        let tree = &mut out[at];
        let Some(id) = screen_id(&s.app, tree.nodes.len()) else { continue };
        tree.nodes.push(filter::screen(id, &s.name, state, filter::readable(&s.nodes, reading)));
    }
    out
}

/// Which of `surfaces` is on top of `stack`. The stack has one entry for a
/// key: the newest open of it is the one showing.
fn top_of(surfaces: &[Surface], stack: &[String]) -> Option<usize> {
    stack.last().and_then(|key| surfaces.iter().rposition(|s| s.key == *key))
}

/// The top screen as it is now.
fn looked_now(surfaces: &[Surface], stack: &[String]) -> Option<Looked> {
    top_of(surfaces, stack).map(|i| Looked { n: surfaces[i].n, rev: filter::rev(&surfaces[i].nodes) })
}

/// A screen's id: its app's, numbered from the second on.
fn screen_id(app: &str, before: usize) -> Option<AgentId> {
    let app: String = app.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let id = if before == 0 { format!("screen.{app}") } else { format!("screen.{app}-{}", before + 1) };
    AgentId::parse(&id).ok()
}

fn reply(r: Response) {
    reply_or(r, || {});
}

/// [`reply`], with what to do when the broker never gets it.
fn reply_or(r: Response, lost: impl FnOnce() + Send + 'static) {
    let Ok(body) = serde_json::to_value(&r) else { return lost() };
    std::thread::spawn(move || {
        if let Err(e) = HttpBroker::configured().request("POST", REPLY, Some(body), Duration::from_secs(5)) {
            log::warn!("agent hub: the reply didn't reach the broker: {}", e.code);
            lost();
        }
    });
}

/// A refusal: the sentence, and whether the person reads it too. A bad id
/// is the model's own mistake, for the model alone.
#[derive(Debug, PartialEq, Eq)]
struct No {
    said: String,
    tell: bool,
}

impl No {
    fn policy(said: impl Into<String>) -> No {
        No { said: said.into(), tell: true }
    }

    fn model(said: impl Into<String>) -> No {
        No { said: said.into(), tell: false }
    }
}

fn refuse(id: String, target: Target, no: No) {
    let mut r = Response::err(id, target, no.said);
    r.result = Some(json!({"tell": no.tell}));
    reply(r);
}

/// What an op asks of its node: the node, and the input.
fn wanted(op: &Op) -> Option<(&AgentId, UiAct)> {
    match op {
        Op::Click { node, .. } => Some((node, UiAct::Press)),
        Op::Type { node, text, .. } => Some((node, UiAct::Type(text.clone()))),
        Op::SetValue { node, value, .. } => Some((node, UiAct::SetValue(value.clone()))),
        _ => None,
    }
}

/// An action that may go ahead.
#[derive(Debug, PartialEq)]
struct Plan {
    /// Which of the surfaces.
    at: usize,
    /// nsh's id of the node.
    node: String,
    /// What the model knows it by.
    label: String,
    act: UiAct,
}

const CHANGED: &str = "The screen changed since the AI last looked at it, so nothing was pressed. It has to look again.";

fn says(s: &str, name: &str) -> String {
    s.replace("{}", name)
}

/// Whether `op` on `target` may go ahead on `surfaces` as they are, or the
/// sentence that says why not. `looked` is the top screen as the model saw
/// it; nothing here reads anything else the model said but the node's id.
fn plan(surfaces: &[Surface], stack: &[String], target: &Target, op: &Op, looked: Option<Looked>, reading: Reading) -> Result<Plan, No> {
    let (id, act) = wanted(op).ok_or_else(|| No::policy("The AI can't do that on a screen, so nothing happened."))?;
    let top = top_of(surfaces, stack).filter(|i| surfaces[*i].target == target.as_str());
    let Some(at) = top else {
        return Err(match surfaces.iter().rfind(|s| s.target == target.as_str()) {
            Some(s) => No::policy(says("{} is covered by another screen or isn't full screen, so nothing was pressed.", &s.name)),
            None => No::model("That mini app isn't open, so nothing was pressed."),
        });
    };
    let s = &surfaces[at];
    if looked != Some(Looked { n: s.n, rev: filter::rev(&s.nodes) }) {
        return Err(No::policy(CHANGED));
    }
    let map = filter::mapped(&s.nodes, reading);
    let Some((node, seen)) = map.iter().find(|(n, _)| n.id == *id) else {
        return Err(No::model("That button or field isn't on this screen any more, so nothing was pressed. The AI has to look again."));
    };
    if node.name.is_empty() {
        return Err(No::policy("That control has no name the AI may read, so it may be a row of someone's data and nothing was pressed. Press it yourself, or name what you want in the chat."));
    }
    let fits = if matches!(act, UiAct::Press) { filter::pressable(&node.role) } else { filter::typable(&node.role) };
    if !fits {
        return Err(No::model(says("\"{}\" doesn't take that action, so nothing happened.", &node.name)));
    }
    if !node.enabled {
        return Err(No::policy(says("\"{}\" is switched off on this screen, so nothing was pressed.", &node.name)));
    }
    if let UiAct::Type(text) | UiAct::SetValue(text) = &act
        && text.chars().any(char::is_control)
    {
        return Err(No::model("The AI can type only plain text, with no line breaks or keys, so nothing was typed."));
    }
    Ok(Plan { at, node: seen.id.clone(), label: node.name.clone(), act })
}

/// Whether a turn of `acting` may act on `s`: only the Bot whose chat opened
/// it, and a plain chat on what a plain chat opened.
fn owned(s: &Surface, acting: &Option<String>) -> Result<(), No> {
    if s.bot.as_ref() == Some(acting) {
        return Ok(());
    }
    Err(No::policy(says("{} was opened from another chat, so this Bot can't act on it and nothing was pressed. Open it from this chat.", &s.name)))
}

/// `surfaces` a turn of `bot` may know of: the rule [`owned`] holds an action to.
fn mine(surfaces: Vec<Surface>, bot: &Option<String>) -> Vec<Surface> {
    surfaces.into_iter().filter(|s| owned(s, bot).is_ok()).collect()
}

/// How an action's failure refuses: a screen that moved is a refusal the person
/// reads too.
fn failed(error: &str, label: &str) -> No {
    if error.starts_with(SCREEN_CHANGED) { No::policy(CHANGED) } else { No::model(unready(error, label)) }
}

/// What an action's failure says: nsh's errors start with a code.
fn unready(error: &str, label: &str) -> String {
    let said = match error.split(':').next().unwrap_or_default() {
        "occluded_by" => "\"{}\" is covered by something else on the screen, so nothing was pressed.",
        "disabled" => "\"{}\" is switched off on this screen, so nothing was pressed.",
        "target_gone" => "\"{}\" went away before it could be pressed, so nothing was pressed.",
        "not_visible" | "offscreen" => "\"{}\" can't be reached on the screen, so nothing was pressed.",
        "focus_missed" => "\"{}\" didn't take the cursor, so nothing was typed.",
        _ => "\"{}\" couldn't be pressed, so nothing happened.",
    };
    says(said, label)
}

/// What the model and the person read for a refused call of an action.
fn refused_line(held: &Held, label: &str) -> String {
    match held {
        Held::Pasteboard => says("\"{}\" copies to the pasteboard, which is a person's to do, so the AI's press was refused and nothing was copied.", label),
        Held::Person => says("\"{}\" needs a person, so the AI's press was refused and nothing was sent. A person has to press it.", label),
        Held::Said(said) | Held::Declined(said) => said.clone(),
    }
}

/// Waits for the calls of an action to be over and quiet.
fn wait_for_calls() {
    let start = Instant::now();
    let mut quiet = Instant::now();
    while start.elapsed() < CALLS_WAIT {
        if attribution::agent_calls() > 0 {
            quiet = Instant::now();
        } else if quiet.elapsed() >= QUIET {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Answers an action, once what it started has shown.
fn finish(id: String, target: Target, label: String, done: Result<(), String>, n: u64, looks: u64, bot: Option<String>) {
    std::thread::spawn(move || {
        wait_for_calls();
        to_store(move |st| {
            let stack: Vec<String> = st.mini_full.with_untracked(|v| v.iter().map(|o| o.key.clone()).collect());
            on_main(move || {
                let current = ACTING.get().is_some_and(|a| a.n == n);
                if current {
                    ACTING.set(None);
                }
                let (declined, refused): (Vec<Held>, Vec<Held>) = press::take_refused().into_iter().partition(|h| matches!(h, Held::Declined(_)));
                let lines = |list: &[Held]| list.iter().map(|held| refused_line(held, &label)).collect::<Vec<String>>();
                let (declined, refused) = (lines(&declined), lines(&refused));
                let held: Vec<_> = press::take_outcomes().into_iter().map(|(tool, outcome)| json!({"tool": tool, "outcome": outcome})).collect();
                let now = mine(surfaces(), &bot);
                if let (Err(e), true) = (&done, refused.is_empty() && declined.is_empty()) {
                    return refuse(id, target, failed(e, &label));
                }
                LOOKED.set(look_after(done.is_ok(), current, LOOKS.get() != looks, LOOKED.get(), looked_now(&now, &stack)));
                let mut r = Response::ok(id, target);
                r.result = Some(json!({"apps": trees(&now, &stack, Reading::configured()), "refused": refused, "declined": declined, "held": held}));
                // the model never saw this screen: it can't go on from it
                reply_or(r, || on_main(|| LOOKED.set(None)));
            });
        });
    });
}

/// Whether open `n` still reads as `rev`. On the main thread.
fn unmoved(n: u64, rev: u64) -> bool {
    surfaces().iter().any(|s| s.n == n && filter::rev(&s.nodes) == rev)
}

/// Runs `op` on `target` for the Bot `bot`, if the screen as it is now lets it.
fn act(id: String, target: Target, op: Op, bot: Option<String>, stack: Vec<String>) {
    if busy(ACTING.get(), Instant::now()) {
        return refuse(id, target, No::model("The AI's last action is still running, so nothing was pressed."));
    }
    let now = surfaces();
    let ready = plan(&now, &stack, &target, &op, LOOKED.get(), Reading::configured()).and_then(|p| owned(&now[p.at], &bot).map(|()| p));
    let view = ready.as_ref().ok().and_then(|p| now[p.at].view.upgrade());
    let (plan, view): (Plan, Rc<AppView>) = match (ready, view) {
        (Ok(plan), Some(view)) => (plan, view),
        (Err(no), _) => return refuse(id, target, no),
        (Ok(_), None) => return refuse(id, target, No::policy(CHANGED)),
    };
    let n = ACTIONS.get() + 1;
    ACTIONS.set(n);
    ACTING.set(Some(Acting { n, since: Instant::now() }));
    press::take_refused();
    press::take_outcomes();
    let (label, looks) = (plan.label.clone(), LOOKS.get());
    let (open, rev) = (now[plan.at].n, filter::rev(&now[plan.at].nodes));
    view.act(&plan.node, plan.act, move || unmoved(open, rev), move |done| finish(id, target, label, done, n, looks, bot));
}

/// Answers `request`, for the agent whatever it says it is. A look or an
/// action goes through the store, for the stack, then the main thread, for
/// the views.
fn answer(request: Envelope) {
    let Envelope { id, target, op, bot, .. } = request;
    if press::authorize_op(&target, &op).is_err() {
        let why = press::op_refused(&target, &op);
        return refuse(id, target, No::policy(why));
    }
    to_store(move |st| {
        let stack: Vec<String> = st.mini_full.with_untracked(|v| v.iter().map(|o| o.key.clone()).collect());
        on_main(move || {
            if !matches!(op, Op::Snapshot { .. }) {
                return act(id, target, op, bot, stack);
            }
            let now = mine(surfaces(), &bot);
            LOOKS.set(LOOKS.get() + 1);
            LOOKED.set(looked_now(&now, &stack));
            let mut r = Response::ok(id, target);
            r.result = Some(json!({"apps": trees(&now, &stack, Reading::configured())}));
            reply(r);
        });
    });
}

/// Whether the card of open `n` still stands over what the person saw:
/// `rev` is the top screen as it went up. Only a screen that moved takes it
/// down: `None` keeps it.
fn card_gone(surfaces: &[Surface], stack: &[String], n: u64, rev: u64) -> Option<Answer> {
    let Some(s) = surfaces.iter().find(|s| s.n == n) else { return Some(Answer::Closed) };
    let on_top = top_of(surfaces, stack).is_some_and(|i| surfaces[i].n == n);
    (!on_top || filter::rev(&s.nodes) != rev).then_some(Answer::Changed)
}

/// How often a card looks at the screen under it.
const CARD_WATCH: Duration = Duration::from_millis(300);
/// How many looks the screen gets to settle: the press itself redraws it (a
/// busy button, a spinner) as the card goes up. Nothing else can act in that
/// time: the action holds the Hub, and the card takes the person's input.
const CARD_SETTLE: u32 = 3;

/// Watches the card of `once` until it is down.
fn watch_card(once: OnceId, n: u64, rev: u64) {
    std::thread::spawn(move || {
        let rev = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(rev));
        for look in 0u32.. {
            std::thread::sleep(CARD_WATCH);
            let (tx, rx) = std::sync::mpsc::channel();
            let (once, rev) = (once.clone(), rev.clone());
            to_store(move |st| {
                let stack: Vec<String> = st.mini_full.with_untracked(|v| v.iter().map(|o| o.key.clone()).collect());
                on_main(move || {
                    let now = surfaces();
                    let seen = now.iter().find(|s| s.n == n).map(|s| filter::rev(&s.nodes));
                    if let Some(seen) = seen.filter(|_| look < CARD_SETTLE) {
                        rev.store(seen, std::sync::atomic::Ordering::Relaxed);
                    }
                    if let Some(gone) = card_gone(&now, &stack, n, rev.load(std::sync::atomic::Ordering::Relaxed)) {
                        card::end(&once, gone);
                    }
                    card::note_scroll(&once);
                    let _ = tx.send(card::is_up(&once));
                });
            });
            if !rx.recv_timeout(Duration::from_secs(5)).unwrap_or(true) {
                return;
            }
        }
    });
}

/// Shows `c` over open `n`, which has to be the top screen. `done` hears how
/// it ended. Any thread.
pub fn show_card(n: u64, once: OnceId, c: Card, done: Box<dyn FnOnce(Answer) + Send>) {
    to_store(move |st| {
        let stack: Vec<String> = st.mini_full.with_untracked(|v| v.iter().map(|o| o.key.clone()).collect());
        on_main(move || {
            let now = surfaces();
            let top = top_of(&now, &stack).filter(|i| now[*i].n == n);
            let view = top.and_then(|i| now[i].view.upgrade());
            let (Some(i), Some(window)) = (top, view.and_then(|v| v.view().window())) else { return done(Answer::Changed) };
            let rev = filter::rev(&now[i].nodes);
            card::show(once.clone(), &c, &window, done);
            watch_card(once, n, rev);
        });
    });
}

/// Takes the card of `once` down, if it is still up. Any thread.
pub fn hide_card(once: OnceId) {
    on_main(move || card::end(&once, Answer::TimedOut));
}

/// Follows the broker's requests for as long as the app runs. This is the
/// Bot's own connection out: nothing here listens.
pub fn start() {
    let run = || {
        let broker = HttpBroker::configured();
        loop {
            let _ = broker.stream(STREAM, |name, data| {
                if name == ui::AGENT_OPENED {
                    attribution::agent_opened();
                } else if name == "ui" {
                    match serde_json::from_str::<Envelope>(data) {
                        Ok(request) => answer(request),
                        Err(_) => log::warn!("agent hub: the broker sent something that is no request"),
                    }
                }
                true
            });
            std::thread::sleep(Duration::from_secs(5));
        }
    };
    if let Err(e) = std::thread::Builder::new().name("buwiz-agent-hub".to_owned()).spawn(run) {
        log::warn!("agent hub: not started: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::filter::tests::{NAME, TIN, button, profile, seen, text};
    use super::press::tests::{click, set, typed};
    use super::*;
    use buwiz_agent::UiNode;

    fn screen_of<'a>(trees: &'a [UiTree], id: &str) -> &'a UiNode {
        trees.iter().flat_map(|t| &t.nodes).find(|n| n.id.as_str() == id).expect("the screen")
    }

    fn surface(key: &str, target: &str, app: &str, name: &str, nodes: Vec<Seen>) -> Surface {
        Surface { n: 7, view: Weak::new(), key: key.into(), target: target.into(), name: name.into(), app: app.into(), bot: Some(None), nodes }
    }

    const FORM: &str = "v1:app.buwiz.ebirforms/taxprofile?ticket=ticket-secret";
    const LIST: &str = "v1:app.buwiz.ebirforms/forms";

    fn stack(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|k| (*k).to_owned()).collect()
    }

    #[test]
    fn each_screen_has_its_name_and_says_where_it_stands() {
        let s = [
            surface(LIST, "ebirforms", "forms", "eBIRForms: Forms", vec![button("n1", "New return")]),
            surface(FORM, "ebirforms", "taxprofile", "eBIRForms: Tax Profile", vec![button("n2", "Create")]),
            surface("v1:app.buwiz.books/ledger", "books", "ledger", "Books: Ledger", vec![]),
        ];
        let got = trees(&s, &stack(&[LIST, FORM]), Reading::Structure);
        assert_eq!(got.iter().map(|t| t.target.as_str()).collect::<Vec<_>>(), ["ebirforms", "books"]);
        let under = screen_of(&got, "screen.forms");
        let top = screen_of(&got, "screen.taxprofile-2");
        assert_eq!((under.name.as_str(), under.states[0].as_str()), ("eBIRForms: Forms", "covered"));
        assert_eq!((top.name.as_str(), top.states[0].as_str()), ("eBIRForms: Tax Profile", "top"));
        assert_eq!(top.children[0].name, "Create");
        assert_eq!(screen_of(&got, "screen.ledger").states[0], "card");
    }

    #[test]
    fn with_nothing_on_the_stack_no_screen_is_on_top() {
        let got = trees(&[surface(LIST, "ebirforms", "forms", "eBIRForms", vec![])], &[], Reading::Structure);
        assert_eq!(screen_of(&got, "screen.forms").states[0], "card");
    }

    #[test]
    fn a_reopened_app_is_the_newest_open_of_its_key() {
        let s = [surface(FORM, "ebirforms", "a", "Old", vec![]), surface(FORM, "ebirforms", "a", "New", vec![])];
        let got = trees(&s, &stack(&[FORM]), Reading::Structure);
        assert_eq!(screen_of(&got, "screen.a").states[0], "covered");
        assert_eq!(screen_of(&got, "screen.a-2").states[0], "top");
    }

    #[test]
    fn a_surface_with_a_bad_target_stays_out() {
        assert!(trees(&[surface(LIST, "Not A Target", "a", "x", vec![])], &[], Reading::Structure).is_empty());
    }

    fn looked(s: &[Surface], stack_: &[String]) -> Option<Looked> {
        looked_now(s, stack_)
    }

    fn books() -> Target {
        Target::parse("books").unwrap()
    }

    const ENTRY: &str = "v1:app.buwiz.books/books";

    fn entry_form() -> Vec<Seen> {
        vec![
            seen("n1", "textfield", "Amount", None, vec![]),
            seen("n2", "textfield", "", None, vec![]),
            button("n3", "Save draft"),
            seen("n4", "button", "", None, vec![text("n5", NAME), text("n6", "Rent")]),
            text("n7", "Behind the scenes"),
        ]
    }

    fn ready(nodes: Vec<Seen>) -> ([Surface; 1], Vec<String>, Option<Looked>) {
        let s = [surface(ENTRY, "books", "books", "Books", nodes)];
        let st = stack(&[ENTRY]);
        let l = looked(&s, &st);
        (s, st, l)
    }

    fn planned(s: &[Surface], st: &[String], op: &Op, l: Option<Looked>) -> Result<Plan, String> {
        plan(s, st, &books(), op, l, Reading::Structure).map_err(|no| no.said)
    }

    #[test]
    fn only_an_action_that_went_through_and_answered_with_the_screen_moves_the_look_on() {
        let (last, now) = (Some(Looked { n: 1, rev: 1 }), Some(Looked { n: 1, rev: 2 }));
        assert_eq!(look_after(true, true, false, last, now), now);
        assert_eq!(look_after(false, true, false, last, now), last, "it failed: no screen went back");
    }

    #[test]
    fn a_look_that_lands_during_an_action_stands() {
        let (seen, now) = (Some(Looked { n: 1, rev: 5 }), Some(Looked { n: 1, rev: 6 }));
        assert_eq!(look_after(true, true, true, seen, now), seen);
    }

    #[test]
    fn an_action_that_lost_its_place_moves_nothing() {
        let (last, now) = (Some(Looked { n: 1, rev: 1 }), Some(Looked { n: 1, rev: 2 }));
        assert_eq!(look_after(true, false, false, last, now), last);
    }

    /// After a failed press the screen moved on: the model never saw this
    /// id, and the look it holds no longer fits.
    #[test]
    fn a_failed_action_leaves_a_later_stale_id_refused() {
        let (_, st, last) = ready(entry_form());
        let mut after = entry_form();
        after.push(button("n9", "Post anyway"));
        let s = [surface(ENTRY, "books", "books", "Books", after)];
        let kept = look_after(false, true, false, last, looked(&s, &st));
        assert_eq!(planned(&s, &st, &click("button.post-anyway"), kept).unwrap_err(), CHANGED);
    }

    #[test]
    fn a_person_typing_between_the_look_and_the_action_refuses() {
        let (_, st, l) = ready(entry_form());
        let mut typed_in = entry_form();
        typed_in[0].value = Some("the person's text".into());
        let s = [surface(ENTRY, "books", "books", "Books", typed_in)];
        assert_eq!(planned(&s, &st, &typed("field.amount", "5"), l).unwrap_err(), CHANGED);
    }

    #[test]
    fn the_models_own_typing_keeps_working_after_its_answer() {
        let (_, st, _) = ready(entry_form());
        let mut after = entry_form();
        after[0].value = Some("1,350.00".into());
        let s = [surface(ENTRY, "books", "books", "Books", after)];
        let l = look_after(true, true, false, None, looked(&s, &st));
        assert!(planned(&s, &st, &set("field.amount", "2"), l).is_ok());
    }

    #[test]
    fn only_the_bot_whose_chat_opened_a_screen_acts_on_it() {
        let hex = Some("hex".to_owned());
        let mut s = surface(ENTRY, "books", "books", "Books", entry_form());
        s.bot = Some(hex.clone());
        assert!(owned(&s, &hex).is_ok());
        let no = owned(&s, &Some("sam".to_owned())).unwrap_err();
        assert!(no.tell && no.said.starts_with("Books was opened from another chat"), "{no:?}");
        assert!(owned(&s, &None).is_err(), "a plain chat on a Bot's screen");
        s.bot = Some(None);
        assert!(owned(&s, &None).is_ok());
        assert!(owned(&s, &hex).is_err(), "a Bot on a plain chat's screen");
        s.bot = None;
        assert!(owned(&s, &None).is_err() && owned(&s, &hex).is_err(), "not known is no");
    }

    fn of(key: &str, target: &str, bot: Option<Option<&str>>) -> Surface {
        let mut s = surface(key, target, target, target, entry_form());
        s.bot = bot.map(|b| b.map(str::to_owned));
        s
    }

    fn seen_by(bot: Option<&str>) -> Vec<String> {
        let s = vec![of(ENTRY, "books", Some(Some("hex"))), of(FORM, "ebirforms", Some(Some("sam"))), of(LIST, "plain", Some(None)), of("v1:x/y", "unknown", None)];
        mine(s, &bot.map(str::to_owned)).iter().map(|s| s.target.clone()).collect()
    }

    /// The same rule as acting: a Bot sees its own, a plain chat its own, and
    /// a screen not known yet is nobody's.
    #[test]
    fn a_look_sees_only_what_its_own_chat_opened() {
        assert_eq!(seen_by(Some("hex")), ["books"]);
        assert_eq!(seen_by(Some("sam")), ["ebirforms"]);
        assert_eq!(seen_by(None), ["plain"]);
        assert!(seen_by(Some("nobody")).is_empty());
    }

    /// The top screen is another Bot's: nothing of it, and nothing to act on.
    #[test]
    fn a_look_under_another_bots_top_screen_gets_no_top() {
        let s = mine(vec![of(ENTRY, "books", Some(Some("hex"))), of(FORM, "ebirforms", Some(Some("sam")))], &Some("hex".to_owned()));
        let st = stack(&[ENTRY, FORM]);
        assert_eq!(looked(&s, &st), None);
        let got = trees(&s, &st, Reading::Structure);
        assert_eq!(got.iter().map(|t| t.target.as_str()).collect::<Vec<_>>(), ["books"]);
        assert_eq!(screen_of(&got, "screen.books").states[0], "covered");
    }

    #[test]
    fn a_screen_that_moved_before_the_press_is_refused_as_changed_and_the_person_reads_it() {
        let no = failed(&format!("{SCREEN_CHANGED}: before the press"), "Post");
        assert_eq!(no, No::policy(CHANGED));
        assert!(!failed("target_gone: n9", "Post").tell);
    }

    fn gone(s: &[Surface], st: &[String], rev: u64) -> Option<&'static str> {
        card_gone(s, st, 7, rev).map(|a| match a {
            Answer::Closed => "closed",
            Answer::Changed => "changed",
            _ => "other",
        })
    }

    #[test]
    fn a_card_stands_only_over_the_screen_as_it_went_up() {
        let (s, st, _) = ready(entry_form());
        let rev = filter::rev(&s[0].nodes);
        assert_eq!(gone(&s, &st, rev), None);
        let mut other = entry_form();
        other[3].children[1] = text("n6", "Utilities");
        let changed = [surface(ENTRY, "books", "books", "Books", other)];
        assert_eq!(gone(&changed, &st, rev), Some("changed"));
        assert_eq!(gone(&[], &st, rev), Some("closed"));
        assert_eq!(gone(&s, &[], rev), Some("changed"), "taken off the stack");
        let mut over = surface(FORM, "ebirforms", "taxprofile", "eBIRForms", vec![]);
        over.n = 8;
        let covered = [surface(ENTRY, "books", "books", "Books", entry_form()), over];
        assert_eq!(gone(&covered, &stack(&[ENTRY, FORM]), rev), Some("changed"), "covered");
    }

    /// The card is no node: a click the Hub is asked for can't name it, on
    /// the Bot's window or on the screen under it.
    #[test]
    fn a_synthesized_click_on_allow_approves_nothing() {
        let _one = hold::tests::serial();
        let once = hold::open(&json!({"entry": "ticket-1"})).unwrap();
        for target in ["bot", "books"] {
            let t = Target::parse(target).unwrap();
            let (s, st, l) = ready(entry_form());
            let refused = press::authorize_op(&t, &click("button.allow")).is_err() || plan(&s, &st, &t, &click("button.allow"), l, Reading::Structure).is_err();
            assert!(refused, "{target}");
        }
        hold::retire(&once);
    }

    #[test]
    fn a_lost_end_stops_holding_the_hub() {
        let t = Instant::now();
        let a = Some(Acting { n: 1, since: t });
        assert!(busy(a, t + ACT_LIMIT - Duration::from_secs(1)));
        assert!(!busy(a, t + ACT_LIMIT));
        assert!(!busy(None, t));
    }

    #[test]
    fn only_a_real_refusal_reaches_the_person() {
        let (s, st, l) = ready(entry_form());
        let tell = |t: &Target, op: &Op, l| plan(&s, &st, t, op, l, Reading::Structure).unwrap_err().tell;
        assert!(!tell(&books(), &click("button.never-was"), l), "an id it made up");
        assert!(!tell(&Target::parse("nosuchapp").unwrap(), &click("button.save-draft"), l), "an app it made up");
        assert!(!tell(&books(), &set("button.save-draft", "x"), l));
        assert!(tell(&books(), &click("button.unnamed-1"), l));
        assert!(tell(&books(), &click("button.save-draft"), None));
    }

    #[test]
    fn a_hub_id_comes_back_as_nshs_node_on_the_top_screen() {
        let (s, st, l) = ready(entry_form());
        let p = planned(&s, &st, &click("button.save-draft"), l).unwrap();
        assert_eq!(p, Plan { at: 0, node: "n3".into(), label: "Save draft".into(), act: UiAct::Press });
        let p = planned(&s, &st, &set("field.amount", "1,350.00"), l).unwrap();
        assert_eq!((p.node.as_str(), p.act), ("n1", UiAct::SetValue("1,350.00".into())));
        let p = planned(&s, &st, &typed("field.amount", "50"), l).unwrap();
        assert_eq!(p.act, UiAct::Type("50".into()));
    }

    #[test]
    fn a_covered_screen_refuses() {
        let s = [surface(ENTRY, "books", "books", "Books", entry_form()), surface(FORM, "ebirforms", "taxprofile", "eBIRForms: Tax Profile", vec![button("n1", "Create")])];
        let st = stack(&[ENTRY, FORM]);
        let l = looked(&s, &st);
        assert_eq!(planned(&s, &st, &click("button.save-draft"), l).unwrap_err(), "Books is covered by another screen or isn't full screen, so nothing was pressed.");
    }

    #[test]
    fn a_card_in_the_chat_is_no_top_screen_and_refuses() {
        let s = [surface(ENTRY, "books", "books", "Books", entry_form())];
        assert!(planned(&s, &[], &click("button.save-draft"), None).unwrap_err().contains("covered by another screen or isn't full screen"));
    }

    #[test]
    fn an_app_that_isnt_open_refuses() {
        let (s, st, l) = ready(entry_form());
        let e = plan(&s, &st, &Target::parse("ebirforms").unwrap(), &click("button.create"), l, Reading::Structure).unwrap_err();
        assert_eq!(e.said, "That mini app isn't open, so nothing was pressed.");
    }

    #[test]
    fn a_stale_id_refuses_rather_than_finding_something_like_it() {
        let (s, st, l) = ready(entry_form());
        assert!(planned(&s, &st, &click("button.save-and-post"), l).unwrap_err().contains("isn't on this screen any more"));
    }

    #[test]
    fn a_screen_that_changed_since_the_look_refuses_even_for_an_id_still_there() {
        let (_, st, l) = ready(entry_form());
        let mut changed = entry_form();
        changed[3].children[0] = text("n5", "Another Fakerson");
        let s = [surface(ENTRY, "books", "books", "Books", changed)];
        assert_eq!(planned(&s, &st, &click("button.save-draft"), l).unwrap_err(), CHANGED);
    }

    /// A list row as a plain group: the entry's text beside its button, not in it.
    fn row(id: &str, entry: &str) -> Seen {
        seen(id, "group", "", None, vec![text(&format!("{id}t"), entry), button(&format!("{id}b"), "Void")])
    }

    #[test]
    fn a_new_entry_beside_an_unchanged_button_refuses() {
        let (_, st, l) = ready(vec![row("r1", "Entry 12"), row("r2", "Entry 13")]);
        let s = [surface(ENTRY, "books", "books", "Books", vec![row("r1", "Entry 14"), row("r2", "Entry 13")])];
        let same: Vec<String> = filter::readable(&s[0].nodes, Reading::Structure).into_iter().map(|n| n.id.to_string()).collect();
        assert_eq!(same, ["button.void", "button.void-2"], "the same to the model");
        assert_eq!(planned(&s, &st, &click("button.void"), l).unwrap_err(), CHANGED);
        assert_eq!(planned(&s, &st, &click("button.void-2"), l).unwrap_err(), CHANGED);
    }

    #[test]
    fn another_open_with_the_same_screen_refuses() {
        let (mut s, st, l) = ready(entry_form());
        s[0].n = 8;
        assert_eq!(planned(&s, &st, &click("button.save-draft"), l).unwrap_err(), CHANGED);
    }

    #[test]
    fn with_no_look_nothing_acts() {
        let (s, st, _) = ready(entry_form());
        assert_eq!(planned(&s, &st, &click("button.save-draft"), None).unwrap_err(), CHANGED);
    }

    #[test]
    fn an_unnamed_node_refuses() {
        let (s, st, l) = ready(entry_form());
        for op in [click("button.unnamed-1"), set("field.unnamed-1", "x")] {
            assert!(planned(&s, &st, &op, l).unwrap_err().starts_with("That control has no name"), "{op:?}");
        }
    }

    #[test]
    fn a_node_the_filter_hid_has_no_id_to_ask_for() {
        let mut nodes = entry_form();
        nodes[2].visible = false;
        let (s, st, l) = ready(nodes);
        assert!(planned(&s, &st, &click("button.save-draft"), l).unwrap_err().contains("isn't on this screen any more"));
        assert!(planned(&s, &st, &click("text.unnamed-1"), l).unwrap_err().contains("isn't on this screen any more"));
    }

    #[test]
    fn a_node_refuses_what_it_doesnt_take() {
        let (s, st, l) = ready(entry_form());
        assert!(planned(&s, &st, &set("button.save-draft", "x"), l).unwrap_err().contains("doesn't take that action"));
        assert!(planned(&s, &st, &click("field.amount"), l).unwrap_err().contains("doesn't take that action"));
    }

    #[test]
    fn a_disabled_node_refuses() {
        let mut nodes = entry_form();
        nodes[2].enabled = false;
        let (s, st, l) = ready(nodes);
        assert!(planned(&s, &st, &click("button.save-draft"), l).unwrap_err().contains("switched off"));
    }

    #[test]
    fn typed_text_cant_be_a_key() {
        let (s, st, l) = ready(entry_form());
        assert!(planned(&s, &st, &typed("field.amount", "5\n"), l).unwrap_err().contains("plain text"));
    }

    #[test]
    fn nshs_errors_become_sentences() {
        for e in ["occluded_by: n3 (a press on n9 lands there)", "target_gone: n9", "focus_missed: the press on n9 did not focus it", "something new"] {
            let said = unready(e, "Save draft");
            assert!(said.starts_with("\"Save draft\"") && !said.contains("n9") && !said.contains('_'), "{said}");
        }
    }

    /// What the broker receives, as bytes.
    #[test]
    fn what_goes_to_the_broker_holds_no_name_tin_or_ticket() {
        let s = [surface(FORM, "ebirforms", "taxprofile", "eBIRForms: Tax Profile", profile())];
        let mut r = Response::ok("ui-1", Target::parse("bot").unwrap());
        r.result = Some(json!({"apps": trees(&s, &stack(&[FORM]), Reading::Structure)}));
        let wire = serde_json::to_string(&r).unwrap();
        assert!(wire.contains("New return") && wire.contains("\"TIN\""));
        for secret in [NAME, "Fakerson", "fakerson", TIN, "000-111", "ticket-secret"] {
            assert!(!wire.contains(secret), "{secret}");
        }
        // and it is a message of the shared protocol, as the broker parses it
        let back: Response = serde_json::from_str(&wire).unwrap();
        let trees: Vec<UiTree> = serde_json::from_value(back.result.unwrap()["apps"].clone()).unwrap();
        assert_eq!(trees[0].target.as_str(), "ebirforms");
    }
}
