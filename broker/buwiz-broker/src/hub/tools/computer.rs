//! The `computer_*` tools: the model's hands on its Bot's computer, for every model provider.
//! They act on the screen of the acting Bot's seat, inside that computer, and never on this Mac.
//!
//! The screen comes from the Bot, and no input names a computer, a screen or a Bot, so these
//! tools can't reach another Bot's screen, and files stay in the Bot's own folder, links followed
//! inside the box. Behind that is a wall: each Bot's commands run as a user of its own, with no way
//! to any screen, browser profile or service, and each Bot's desktop as another. On a shared
//! computer `computer_run` still goes only when a check made from outside says so
//! ([`buwiz_computer::Provider::users_apart`]), since nothing the box says of itself counts.
//!
//! One switch, `computer`, on by default for a Bot: unlike the shell, what runs here is walled off
//! from the person's Mac and files. What keeps a person in charge is that the model can't make or
//! wake one: the computer must be awake, by a person's click, or the tools refuse. At Ask, what
//! changes something waits for a card. Off, the tools are gone.
//!
//! A routine is different, because no one is there to click: its turn has these tools only when
//! a person turned `computer_routines` on, which starts off and which only a person can widen. It
//! may then wake the computer. At Ask it is refused, since nobody can answer the card.
//!
//! Pictures are what these tools run on, so a model that can't see gets none of them, and is told
//! why ([`prompt`]). They go as JPEG, and a turn keeps only the latest few (`agent/turn.rs`).
//!
//! A person comes first. While they drive this Bot's screen, or while it's in their hands, every
//! tool here waits, the ones that only look included: what they're doing isn't for the model. Past
//! `PATIENCE` a call gives up with nothing done. The picture at the end of an action is held back
//! the same way. A call can still land in the moment after the person's first input, and a
//! `computer_run` under way keeps going; a person's input never waits for the agent.
//!
//! The first run to use a screen keeps it until it ends: a second run of the same Bot is refused
//! ([`crate::hub::computer::Hold`]). An agent working for another isn't given these tools at all
//! ([`delegate`]): the computer is its own Bot's, for its own person's work.
//!
//! `computer_ask_person` hands the screen over: a card, then the person on this Bot's own screen,
//! where its browser is its own profile, until they hand it back or the time runs out. That
//! keeps the password out of the chat and the model's pictures. Commands can't see that screen, so
//! they carry on. What the model got running on the screen itself, by clicks, runs as the desktop's
//! user, and that is ended first ([`buwiz_computer::Provider::clear_strays`]), and the model told
//! what went. Ended and not stopped: a stopped keylogger reads the missed keys when it's resumed.
//! That part isn't a wall, and a page in the browser is a page. So the card says so, and asks for
//! trust in plain words.
//!
//! What these tools read is outside content (a page on that screen can say anything): it counts as
//! such for the switches that ask after outside content.

use base64::Engine;
use buwiz_computer::{Input, Problem, Settled, WaitFor};
use serde_json::{json, Value};

use super::{Out, Run};
use crate::hub::computer::{Computers, Ended, Live};
use crate::hub::library::botctx;
use crate::hub::policy::ask::{self as asking, Asking};
use crate::hub::policy::{self, Pos};
use crate::js;

pub const KEY: &str = "computer";
/// A routine may use the computer: off until a person turns it on.
pub const ROUTINES_KEY: &str = "computer_routines";
pub const ASK_PERSON: &str = "computer_ask_person";
/// The card a hand-over asks on: never `computer`, whose "Allow in this chat" would widen the rest.
pub const ASK_KEY: &str = "computer_takeover";
pub const NAMES: [&str; 13] = [
    "computer_screenshot",
    "computer_click",
    "computer_drag",
    "computer_type",
    "computer_key",
    "computer_scroll",
    "computer_run",
    "computer_open_url",
    "computer_wait",
    "computer_read_file",
    "computer_write_file",
    "computer_record",
    ASK_PERSON,
];
/// The ones that don't need a card first. `computer_ask_person` is its own card.
pub const READS: [&str; 4] = ["computer_screenshot", "computer_wait", "computer_read_file", ASK_PERSON];

/// How long a person gets, when the model doesn't say, and the most and least it can say.
const TAKEOVER_S: u64 = 300;
const TAKEOVER_MIN_S: u64 = 30;
const TAKEOVER_MAX_S: u64 = 1800;
const MAX_REASON: usize = 200;
const WAITING_FOR_PERSON: &str = "Waiting for a person on its computer";
pub const TAKEOVER_NOTE: &str = "You'd use its own browser on its own screen. The AI isn't shown your screen and can't act through its tools until you hand back. Commands it ran can keep running, but they can't see this screen or what you type. The browser and desktop here are a different matter: pages it opened, and anything it set going by clicking on this screen, could. What we can find on the desktop is ended first, not always all of it. Don't type a real password unless you trust this Bot's work so far. Afterwards it can use whatever you signed in to.";

/// What a command's or a file's text may be, for the model.
const MAX_TEXT: usize = 30_000;

/// The tools' schemas.
pub fn defs() -> Vec<Value> {
    let point = json!({"x": {"type": "integer", "description": "pixels from the left of the screen"}, "y": {"type": "integer", "description": "pixels from the top"}});
    let with = |extra: Value| {
        let mut p = point.clone();
        if let (Some(p), Some(e)) = (p.as_object_mut(), extra.as_object()) {
            p.extend(e.clone());
        }
        p
    };
    let tail = " Answers with the settled screen.";
    vec![
        json!({"name": "computer_screenshot", "description": "See your own screen on your Bot's computer, a Linux desktop kept apart from this Mac. If the computer is asleep only a person can wake it.", "input_schema": {"type": "object", "properties": {}}}),
        json!({"name": "computer_click", "description": format!("Click on your screen on your computer.{tail}"), "input_schema": {"type": "object", "properties": with(json!({"button": {"type": "string", "enum": ["left", "middle", "right"]}, "double": {"type": "boolean"}})), "required": ["x", "y"]}}),
        json!({"name": "computer_drag", "description": format!("Drag from one point to another on your screen.{tail}"), "input_schema": {"type": "object", "properties": {"x1": {"type": "integer"}, "y1": {"type": "integer"}, "x2": {"type": "integer"}, "y2": {"type": "integer"}}, "required": ["x1", "y1", "x2", "y2"]}}),
        json!({"name": "computer_type", "description": format!("Type text into what has focus on your screen.{tail}"), "input_schema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}}),
        json!({"name": "computer_key", "description": format!("Press a key or chord on your screen, as an X keysym: Return, Tab, ctrl+l.{tail}"), "input_schema": {"type": "object", "properties": {"key": {"type": "string"}}, "required": ["key"]}}),
        json!({"name": "computer_scroll", "description": format!("Scroll at a point on your screen. dy positive scrolls down; 120 is one notch.{tail}"), "input_schema": {"type": "object", "properties": with(json!({"dx": {"type": "integer"}, "dy": {"type": "integer"}})), "required": ["x", "y"]}}),
        json!({"name": "computer_run", "description": "Run a shell command on your computer, in your own folder there, and get its output. It runs in that computer, not on this Mac, as a user of its own: it has no display and can't reach your screen or browser, so use the other computer tools for those. Ports below 6906 aren't open to it.", "input_schema": {"type": "object", "properties": {"command": {"type": "string"}, "timeout_s": {"type": "integer", "description": "at most 600; 30 when left out"}}, "required": ["command"]}}),
        json!({"name": "computer_open_url", "description": format!("Open an http or https page in your screen's browser.{tail}"), "input_schema": {"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}}),
        json!({"name": "computer_wait", "description": format!("Wait until your screen has been still for idle_ms (300 when left out), up to timeout_ms.{tail}"), "input_schema": {"type": "object", "properties": {"idle_ms": {"type": "integer"}, "timeout_ms": {"type": "integer"}}}}),
        json!({"name": "computer_read_file", "description": "Read a text file from your own folder on your computer. A path is relative to that folder.", "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}),
        json!({"name": "computer_write_file", "description": "Write a text file in your own folder on your computer, making its folders. A path is relative to that folder.", "input_schema": {"type": "object", "properties": {"path": {"type": "string"}, "content": {"type": "string"}}, "required": ["path", "content"]}}),
        json!({"name": "computer_record", "description": "Start recording your screen, or stop and keep the MP4 in your folder on your computer. stop needs the id start gave.", "input_schema": {"type": "object", "properties": {"action": {"type": "string", "enum": ["start", "stop"]}, "id": {"type": "integer"}}, "required": ["action"]}}),
        json!({"name": ASK_PERSON, "description": format!("Ask a person to take over your screen for what only they should do, such as signing in or a one-time code: open the page first, and they use your own browser there. Your commands keep running; whatever you left running on the screen's own desktop is ended first. You wait until they hand it back or the time runs out, then you get the screen as they left it. Never ask for a password in chat.{tail}"), "input_schema": {"type": "object", "properties": {"reason": {"type": "string", "description": "what you need them to do, in one line"}, "time_limit_s": {"type": "integer", "description": "how long they have once they say yes: 30 to 1800; 300 when left out"}}, "required": ["reason"]}}),
    ]
}

/// The tools this turn is offered: a turn nobody can answer can't ask a person.
pub fn defs_for(run: &Run) -> Vec<Value> {
    let present = asking::present(&run.turn);
    defs().into_iter().filter(|d| present || d["name"] != ASK_PERSON).collect()
}

/// The Bot this turn acts for, when it has one.
fn bot(run: &Run) -> Option<String> {
    run.ctx.store.read(|s| botctx::bot_of(s, &run.turn).map(|b| b.id.clone()))
}

/// Whether this turn is an agent working for another: a request from one, or a temporary helper.
/// A delegate gets no computer tool, whoever's screen it would act on. Not knowing counts as one.
fn delegate(run: &Run) -> bool {
    run.turn.acting_bot.is_some() || run.ctx.store.read(|s| botctx::bot_of(s, &run.turn).is_none_or(|b| b.helper.is_some()))
}

/// Whether a routine's turn may have them.
fn routine_allowed(run: &Run, bot: &str) -> bool {
    run.turn.routine.is_none() || policy::tool_state(&run.ctx, bot, ROUTINES_KEY) == Pos::On
}

/// The turn's model, as a catalog names it.
fn model_of(run: &Run) -> String {
    run.turn.model.as_ref().map(|m| m.model.clone()).filter(|m| !m.is_empty()).unwrap_or_else(|| run.settings.model.clone())
}

/// Whether this turn's model sees images. A model the catalog doesn't name counts as seeing, as
/// does any when there is no catalog: a key straight to a provider has none, and refusing there
/// would lose the computer on models that can use it.
async fn sees(run: &Run) -> bool {
    match Computers::of(&run.ctx) {
        Some(c) => c.sees(&run.settings.opencodex, &model_of(run)).await != Some(false),
        None => false,
    }
}

const BLIND: &str = "This model can't see images, so it can't use the computer. Pick a model that sees images for this Bot.";

/// Whether this turn is offered the tools: its Bot sits at a computer, it isn't a routine nobody
/// let in, and its model can see the screen. A computer nobody made has nothing to offer.
pub async fn offered(run: &Run) -> bool {
    seated(run) && sees(run).await
}

/// Whether this turn's Bot sits at a computer it may use from here.
fn seated(run: &Run) -> bool {
    !delegate(run) && bot(run).is_some_and(|b| matches!(Computers::seat(&run.ctx, &b), Ok(Some(_))) && routine_allowed(run, &b))
}

/// What the system prompt says of the computer: how to use it, or why this model can't.
pub async fn prompt(run: &Run) -> String {
    if !seated(run) {
        String::new()
    } else if sees(run).await {
        "\n\nYour computer: the screen is for acting. To read a page or a file, use a text route where one exists (fetch_url, computer_read_file, computer_run), because each screenshot is costly and only the latest few stay with you. When a page needs a person, such as a sign-in, use computer_ask_person rather than asking for a password.".to_owned()
    } else {
        format!("\n\nYour computer: if asked to use it, say this in your own words: {BLIND}")
    }
}

fn no(problem: &Problem) -> Out {
    Out::err(problem.sentence())
}

fn num(input: &Value, key: &str) -> Option<i32> {
    input.get(key).and_then(Value::as_f64).map(|n| n as i32)
}

fn text<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str)
}

/// `path` inside `folder`, the Bot's own on the computer; `None` for one that leaves it.
pub fn own_path(folder: &str, path: &str) -> Option<String> {
    let rest = path.strip_prefix(&format!("/workspace/{folder}/")).or_else(|| path.strip_prefix(&format!("{folder}/"))).unwrap_or(path);
    let safe = !rest.is_empty() && !rest.starts_with('/') && rest.split('/').all(|p| !matches!(p, "" | "." | ".."));
    safe.then(|| format!("{folder}/{rest}"))
}

/// `path` in `folder` as the computer resolves it, links followed; `None` when that leaves the
/// folder.
async fn inside(live: &Live, path: &str) -> Result<Option<String>, Problem> {
    let Some(rel) = own_path(&live.screen.folder, path) else { return Ok(None) };
    let real = live.provider.resolve(&live.machine, &rel).await?;
    Ok(real.starts_with(&format!("/workspace/{}/", live.screen.folder)).then_some(real))
}

/// The screen as blocks: the facts for the model, then the picture. The picture waits for a person
/// who came in meanwhile, and is left out if they stay.
async fn look(run: &Run, live: &Live, facts: Value) -> Out {
    if let Some(computers) = Computers::of(&run.ctx) {
        if let Err(p) = wait_for_person(run, &computers, &live.bot).await {
            return Out::ok(live.machine.scrub(&js::stringify(&json!({"facts": facts, "picture": p.sentence()}))));
        }
    }
    match live.provider.screenshot(&live.machine, &live.screen).await {
        Ok(shot) => {
            let (bytes, media) = shot.for_model();
            let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let facts = live.machine.scrub(&js::stringify(&facts));
            Out { content: json!([{"type": "text", "text": facts}, {"type": "image", "source": {"type": "base64", "media_type": media, "data": data}}]), is_error: None }
        }
        Err(p) => no(&p),
    }
}

/// `act`, then what it led to, settled.
async fn acted(run: &Run, live: &Live, act: impl std::future::Future<Output = Result<(), Problem>>, since: Option<u64>) -> Out {
    if let Err(p) = act.await {
        return no(&p);
    }
    let settled = live.provider.settle(&live.machine, &live.screen, WaitFor { since, ..WaitFor::default() }).await.unwrap_or(Settled { fallback: true, ..Settled::default() });
    look(run, live, json!({"changed": settled.changed, "waited_ms": settled.waited_ms, "timed_out": settled.timed_out})).await
}

async fn wait_for_person(run: &Run, computers: &Computers, bot: &str) -> Result<bool, Problem> {
    if !computers.person_on(bot) {
        return Ok(false);
    }
    run.set_job("waiting", WAITING_FOR_PERSON);
    let waited = computers.person_gone(bot).await;
    run.set_job("connecting", "Working on its computer");
    waited
}

/// `live`, and the mark to act from, once nobody is on the screen. Reading the mark takes a moment,
/// so it's read first and the person looked for last: only an action sent right after this can
/// land on their first input.
async fn clear_to_act(run: &Run, computers: &Computers, live: Live, name: &str) -> Result<(Live, Option<u64>), Problem> {
    let mut live = live;
    loop {
        if wait_for_person(run, computers, &live.bot).await? {
            // it may have slept or moved while it waited
            live = computers.live(&run.ctx, &live.bot).await?;
        }
        let looks_alone = matches!(name, "computer_screenshot" | "computer_wait" | "computer_run" | "computer_read_file" | "computer_write_file" | "computer_record" | ASK_PERSON);
        let since = if looks_alone { None } else { live.provider.mark(&live.machine, &live.screen).await };
        if !computers.person_on(&live.bot) {
            return Ok((live, since));
        }
    }
}

/// This run's claim on the screen: the first to touch it has it until it ends.
fn hold_screen(run: &Run, computers: &std::sync::Arc<Computers>, live: &Live) -> Result<(), Problem> {
    let mut state = run.state();
    if !state.screen_hold.as_ref().is_some_and(|h| h.is_for(&live.bot)) {
        state.screen_hold = Some(computers.hold(&live.bot, &live.computer)?);
    }
    Ok(())
}

/// `computer_ask_person`: a card, then the screen is the person's until it comes back.
async fn ask_person(run: &Run, computers: &std::sync::Arc<Computers>, live: Live, input: &Value) -> Out {
    let Some(reason) = text(input, "reason").map(str::trim).filter(|r| !r.is_empty()) else { return Out::err("computer_ask_person needs a reason".to_owned()) };
    let reason = js::slice_utf16(reason, MAX_REASON).to_owned();
    let limit = input.get("time_limit_s").and_then(Value::as_u64).unwrap_or(TAKEOVER_S).clamp(TAKEOVER_MIN_S, TAKEOVER_MAX_S);
    let name = run.ctx.store.read(|s| s.bot(&live.bot).map(|b| b.name.clone())).unwrap_or_default();
    let asked = Asking { bot: live.bot.clone(), key: ASK_KEY.to_owned(), tool: ASK_PERSON.to_owned(), title: format!("Take over {name}'s computer?"), lines: vec![reason.clone(), TAKEOVER_NOTE.to_owned()], ..Asking::default() };
    if let Err(no_answer) = asking::ask(run, asked).await {
        return Out::err(no_answer);
    }
    // the answer can take minutes: what was seen before it is no use now
    let live = match computers.live(&run.ctx, &live.bot).await {
        Ok(l) => l,
        Err(p) => return no(&p),
    };
    let recording = match computers.end_recording(&live).await {
        Ok(r) => r,
        Err(p) => return no(&p),
    };
    // the card says commands can't see this screen, so that's checked, from outside, each time
    if !matches!(live.provider.users_apart(&live.machine, std::slice::from_ref(&live.bot)).await, Ok(true)) {
        return Out::err("This computer's commands can't be confirmed walled off from its screen right now, so nothing was handed over.".to_owned());
    }
    // nothing is handed over beside what can't be cleared: this Bot's desktop alone
    let ended_programs = match live.provider.clear_strays(&live.machine, &live.screen).await {
        Ok(names) => names,
        Err(p) => return no(&p),
    };
    let mut handing = match computers.hand_over(&live, &reason, std::time::Duration::from_secs(limit)) {
        Ok(h) => h,
        Err(p) => return no(&p),
    };
    run.set_job("waiting", asking::WAITING);
    let ended = handing.ended().await;
    drop(handing);
    run.set_job("connecting", "Working on its computer");
    let outcome = match ended {
        Some(Ended::HandedBack) => "handed_back",
        None => "timed_out",
        Some(Ended::Asleep) => return no(&Problem::Asleep),
        Some(Ended::Gone) => return no(&Problem::Gone),
    };
    Computers::touch(&run.ctx, &live.computer);
    let live = match computers.live(&run.ctx, &live.bot).await {
        Ok(l) => l,
        Err(p) => return no(&p),
    };
    let mut facts = json!({"outcome": outcome});
    if outcome == "timed_out" {
        facts["note"] = json!("Their time ran out, so they may not have finished.");
    }
    if let Some(path) = recording {
        facts["recording_stopped"] = json!(path);
    }
    if !ended_programs.is_empty() {
        facts["programs_ended"] = json!(ended_programs);
        facts["programs_note"] = json!("What was left running on your screen's desktop was ended before the person started. Start again what you still need.");
    }
    look(run, &live, facts).await
}

/// One of [`NAMES`]. The switches have had their say (`controls`).
pub async fn tool(run: &Run, name: &str, input: &Value) -> Out {
    let Some(bot) = bot(run) else { return Out::err("A computer belongs to a Bot, and this chat has none.".to_owned()) };
    let Some(computers) = Computers::of(&run.ctx) else { return Out::err("This broker has no computers.".to_owned()) };
    if delegate(run) {
        return Out::err("A computer is for its own Bot's work: an agent working for another can't use it.".to_owned());
    }
    if !routine_allowed(run, &bot) {
        return Out::err("A routine can use the computer only when a person turns that on.".to_owned());
    }
    if !sees(run).await {
        return Out::err(BLIND.to_owned());
    }
    let ctx = &run.ctx;
    run.set_job("connecting", "Working on its computer");
    let mut live = computers.live(ctx, &bot).await;
    if run.turn.routine.is_some() && matches!(live, Err(Problem::Asleep)) {
        if let Ok(Some(seat)) = Computers::seat(ctx, &bot) {
            if let Err(p) = computers.wake_and_wait(ctx, &seat.computer).await {
                return no(&p);
            }
            live = computers.live(ctx, &bot).await;
        }
    }
    let live = match live {
        Ok(l) => l,
        Err(p) => return no(&p),
    };
    if let Err(p) = hold_screen(run, &computers, &live) {
        return no(&p);
    }
    let (live, since) = match clear_to_act(run, &computers, live, name).await {
        Ok(cleared) => cleared,
        Err(p) => return no(&p),
    };
    let computer = live.computer.clone();
    Computers::touch(ctx, &computer);
    let out = if name == ASK_PERSON { ask_person(run, &computers, live, input).await } else { act(run, &computers, &live, name, input, since).await };
    Computers::touch(ctx, &computer);
    out
}

async fn act(run: &Run, computers: &Computers, live: &Live, name: &str, input: &Value, since: Option<u64>) -> Out {
    let (provider, machine, screen) = (&live.provider, &live.machine, &live.screen);
    let hands = |i: Input| async move { provider.input(machine, screen, &i).await };
    match name {
        "computer_screenshot" => look(run, live, json!({})).await,
        "computer_click" => {
            let (Some(x), Some(y)) = (num(input, "x"), num(input, "y")) else { return Out::err("computer_click needs x and y".to_owned()) };
            let button = match text(input, "button") {
                Some("middle") => 2,
                Some("right") => 3,
                _ => 1,
            };
            let double = input.get("double").and_then(Value::as_bool).unwrap_or(false);
            acted(run, live, hands(Input::Click { x, y, button, double }), since).await
        }
        "computer_drag" => {
            let (Some(x1), Some(y1), Some(x2), Some(y2)) = (num(input, "x1"), num(input, "y1"), num(input, "x2"), num(input, "y2")) else { return Out::err("computer_drag needs x1, y1, x2 and y2".to_owned()) };
            acted(run, live, hands(Input::Drag { from: (x1, y1), to: (x2, y2) }), since).await
        }
        "computer_type" => {
            let Some(t) = text(input, "text") else { return Out::err("computer_type needs text".to_owned()) };
            acted(run, live, hands(Input::Type { text: t.to_owned() }), since).await
        }
        "computer_key" => {
            let Some(k) = text(input, "key") else { return Out::err("computer_key needs key".to_owned()) };
            acted(run, live, hands(Input::Key { key: k.to_owned() }), since).await
        }
        "computer_scroll" => {
            let (Some(x), Some(y)) = (num(input, "x"), num(input, "y")) else { return Out::err("computer_scroll needs x and y".to_owned()) };
            let (dx, dy) = (num(input, "dx").unwrap_or(0), num(input, "dy").unwrap_or(0));
            acted(run, live, hands(Input::Scroll { x, y, dx, dy }), since).await
        }
        "computer_run" => {
            let Some(command) = text(input, "command") else { return Out::err("computer_run needs command".to_owned()) };
            // on a shared computer the other Bots are what's at stake, so this is read from outside
            // for each command: a failed read is a no, and nothing the box says counts
            let seats = match Computers::seats(&run.ctx, &live.computer) {
                Ok(seats) => seats,
                Err(p) => return no(&p),
            };
            if seats.len() > 1 {
                let bots: Vec<String> = seats.into_iter().map(|s| s.bot).collect();
                if !matches!(provider.users_apart(machine, &bots).await, Ok(true)) {
                    return Out::err("computer_run is off on a computer several Bots share until they're confirmed apart there, and they aren't now. Use the screen, or ask for a computer of your own.".to_owned());
                }
            }
            let timeout = input.get("timeout_s").and_then(Value::as_u64).unwrap_or(30).clamp(1, 600);
            match provider.run(machine, screen, command, timeout * 1000).await {
                Ok(o) => {
                    let body = json!({"exit_code": o.exit_code, "stdout": js::slice_utf16(&o.stdout, MAX_TEXT), "stderr": js::slice_utf16(&o.stderr, MAX_TEXT), "truncated": o.truncated, "timed_out": o.timed_out});
                    Out::ok(machine.scrub(&js::stringify(&body)))
                }
                Err(p) => no(&p),
            }
        }
        "computer_open_url" => {
            let Some(url) = text(input, "url").filter(|u| u.starts_with("http://") || u.starts_with("https://")) else { return Out::err("computer_open_url takes an http or https URL".to_owned()) };
            let since = provider.mark(machine, screen).await;
            acted(run, live, provider.open_url(machine, screen, url), since).await
        }
        "computer_wait" => {
            let idle_ms = input.get("idle_ms").and_then(Value::as_u64).unwrap_or(300).clamp(50, 10_000);
            let timeout_ms = input.get("timeout_ms").and_then(Value::as_u64).unwrap_or(8000).clamp(100, 60_000);
            match provider.settle(machine, screen, WaitFor { idle_ms, timeout_ms, since: None }).await {
                Ok(s) => look(run, live, json!({"changed": s.changed, "waited_ms": s.waited_ms, "timed_out": s.timed_out})).await,
                Err(p) => no(&p),
            }
        }
        "computer_read_file" => {
            let path = match inside(live, text(input, "path").unwrap_or_default()).await {
                Ok(Some(p)) => p,
                Ok(None) => return Out::err("computer_read_file reads inside your own folder on the computer".to_owned()),
                Err(p) => return no(&p),
            };
            match provider.read_file(machine, &path).await {
                Ok(bytes) => Out::ok(machine.scrub(&js::slice_utf16(&String::from_utf8_lossy(&bytes), MAX_TEXT))),
                Err(p) => no(&p),
            }
        }
        "computer_write_file" => {
            let path = match inside(live, text(input, "path").unwrap_or_default()).await {
                Ok(Some(p)) => p,
                Ok(None) => return Out::err("computer_write_file writes inside your own folder on the computer".to_owned()),
                Err(p) => return no(&p),
            };
            let content = text(input, "content").unwrap_or_default();
            match provider.write_file(machine, &path, content.as_bytes()).await {
                Ok(()) => Out::ok(js::stringify(&json!({"saved": path, "bytes": content.len()}))),
                Err(p) => no(&p),
            }
        }
        "computer_record" => match text(input, "action") {
            Some("start") => {
                if let Err(why) = computers.begin_recording(live) {
                    return Out::err(why.to_owned());
                }
                match provider.record_start(machine, screen).await {
                    Ok(id) if computers.recording_started(&live.bot, id) => Out::ok(js::stringify(&json!({"recording": id}))),
                    Ok(id) => {
                        let stopped = provider.record_stop(machine, screen, id).await;
                        computers.recording_stopped(&live.bot, stopped.is_ok());
                        Out::err("A person came to this screen as the recording started, so it was stopped.".to_owned())
                    }
                    Err(p) => {
                        computers.recording_failed(&live.bot);
                        no(&p)
                    }
                }
            }
            Some("stop") => {
                let Some(id) = input.get("id").and_then(Value::as_u64) else { return Out::err("computer_record stop needs the id start gave".to_owned()) };
                match provider.record_stop(machine, screen, id).await {
                    Ok(path) => {
                        computers.model_stopped_recording(&live.bot, id);
                        Out::ok(js::stringify(&json!({"saved": path})))
                    }
                    Err(p) => no(&p),
                }
            }
            _ => Out::err("computer_record's action is start or stop".to_owned()),
        },
        _ => Out::err(format!("unknown tool {name}")),
    }
}
