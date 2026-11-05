//! The `computer_*` tools: the model's hands on its Bot's computer, for every model provider.
//! They act on the screen of the acting Bot's seat, inside that computer, and never on this Mac.
//!
//! The screen comes from the Bot, and no input names a computer, a screen or a Bot, so these
//! tools can't reach another Bot's screen, and files stay in the Bot's own folder. That is the
//! tools' reach, not a wall: inside a box every screen is one user, so anything running a command
//! there can reach them all. So `computer_run` is off on a shared computer until the box says its
//! commands stay on their screen, and work that must stay apart needs separate computers.
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
//! What these tools read is outside content (a page on that screen can say anything): it counts as
//! such for the switches that ask after outside content.

use base64::Engine;
use buwiz_computer::{Input, Problem, Settled, WaitFor};
use serde_json::{json, Value};

use super::{Out, Run};
use crate::hub::computer::{Computers, Live};
use crate::hub::library::botctx;
use crate::hub::policy::{self, Pos};
use crate::js;

pub const KEY: &str = "computer";
/// A routine may use the computer: off until a person turns it on.
pub const ROUTINES_KEY: &str = "computer_routines";
pub const NAMES: [&str; 12] = [
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
];
/// The ones that change nothing.
pub const READS: [&str; 3] = ["computer_screenshot", "computer_wait", "computer_read_file"];

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
        json!({"name": "computer_run", "description": "Run a shell command on your computer, in your own folder there, and get its output. It runs in that computer, not on this Mac.", "input_schema": {"type": "object", "properties": {"command": {"type": "string"}, "timeout_s": {"type": "integer", "description": "at most 600; 30 when left out"}}, "required": ["command"]}}),
        json!({"name": "computer_open_url", "description": format!("Open an http or https page in your screen's browser.{tail}"), "input_schema": {"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}}),
        json!({"name": "computer_wait", "description": format!("Wait until your screen has been still for idle_ms (300 when left out), up to timeout_ms.{tail}"), "input_schema": {"type": "object", "properties": {"idle_ms": {"type": "integer"}, "timeout_ms": {"type": "integer"}}}}),
        json!({"name": "computer_read_file", "description": "Read a text file from your own folder on your computer. A path is relative to that folder.", "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}),
        json!({"name": "computer_write_file", "description": "Write a text file in your own folder on your computer, making its folders. A path is relative to that folder.", "input_schema": {"type": "object", "properties": {"path": {"type": "string"}, "content": {"type": "string"}}, "required": ["path", "content"]}}),
        json!({"name": "computer_record", "description": "Start recording your screen, or stop and keep the MP4 in your folder on your computer. stop needs the id start gave.", "input_schema": {"type": "object", "properties": {"action": {"type": "string", "enum": ["start", "stop"]}, "id": {"type": "integer"}}, "required": ["action"]}}),
    ]
}

/// The Bot this turn acts for, when it has one.
fn bot(run: &Run) -> Option<String> {
    run.ctx.store.read(|s| botctx::bot_of(s, &run.turn).map(|b| b.id.clone()))
}

/// Whether a routine's turn may have them.
fn routine_allowed(run: &Run, bot: &str) -> bool {
    run.turn.routine.is_none() || policy::tool_state(&run.ctx, bot, ROUTINES_KEY) == Pos::On
}

/// Whether this turn is offered the tools: its Bot sits at a computer, and it isn't a routine
/// nobody let in. A computer nobody made has nothing to offer.
pub fn offered(run: &Run) -> bool {
    bot(run).is_some_and(|b| Computers::seat(&run.ctx, &b).is_some() && routine_allowed(run, &b))
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

/// The screen as blocks: the facts for the model, then the picture.
async fn look(live: &Live, facts: Value) -> Out {
    match live.provider.screenshot(&live.machine, &live.screen).await {
        Ok(shot) => {
            let data = base64::engine::general_purpose::STANDARD.encode(&shot.png);
            let facts = live.machine.scrub(&js::stringify(&facts));
            Out { content: json!([{"type": "text", "text": facts}, {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": data}}]), is_error: None }
        }
        Err(p) => no(&p),
    }
}

/// `act`, then what it led to, settled.
async fn acted(live: &Live, act: impl std::future::Future<Output = Result<(), Problem>>, since: Option<u64>) -> Out {
    if let Err(p) = act.await {
        return no(&p);
    }
    let settled = live.provider.settle(&live.machine, &live.screen, WaitFor { since, ..WaitFor::default() }).await.unwrap_or(Settled { fallback: true, ..Settled::default() });
    look(live, json!({"changed": settled.changed, "waited_ms": settled.waited_ms, "timed_out": settled.timed_out})).await
}

/// One of [`NAMES`]. The switches have had their say (`controls`).
pub async fn tool(run: &Run, name: &str, input: &Value) -> Out {
    let Some(bot) = bot(run) else { return Out::err("A computer belongs to a Bot, and this chat has none.".to_owned()) };
    let Some(computers) = Computers::of(&run.ctx) else { return Out::err("This broker has no computers.".to_owned()) };
    if !routine_allowed(run, &bot) {
        return Out::err("A routine can use the computer only when a person turns that on.".to_owned());
    }
    let ctx = &run.ctx;
    run.set_job("connecting", "Working on its computer");
    let mut live = computers.live(ctx, &bot).await;
    if run.turn.routine.is_some() && matches!(live, Err(Problem::Asleep)) {
        if let Some(seat) = Computers::seat(ctx, &bot) {
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
    Computers::touch(ctx, &live.computer);
    let out = act(run, &live, name, input).await;
    Computers::touch(ctx, &live.computer);
    out
}

async fn act(run: &Run, live: &Live, name: &str, input: &Value) -> Out {
    let (provider, machine, screen) = (&live.provider, &live.machine, &live.screen);
    let hands = |i: Input| async move { provider.input(machine, screen, &i).await };
    let since = if matches!(name, "computer_screenshot" | "computer_wait" | "computer_run" | "computer_read_file" | "computer_write_file" | "computer_record") {
        None
    } else {
        provider.mark(machine, screen).await
    };
    match name {
        "computer_screenshot" => look(live, json!({})).await,
        "computer_click" => {
            let (Some(x), Some(y)) = (num(input, "x"), num(input, "y")) else { return Out::err("computer_click needs x and y".to_owned()) };
            let button = match text(input, "button") {
                Some("middle") => 2,
                Some("right") => 3,
                _ => 1,
            };
            let double = input.get("double").and_then(Value::as_bool).unwrap_or(false);
            acted(live, hands(Input::Click { x, y, button, double }), since).await
        }
        "computer_drag" => {
            let (Some(x1), Some(y1), Some(x2), Some(y2)) = (num(input, "x1"), num(input, "y1"), num(input, "x2"), num(input, "y2")) else { return Out::err("computer_drag needs x1, y1, x2 and y2".to_owned()) };
            acted(live, hands(Input::Drag { from: (x1, y1), to: (x2, y2) }), since).await
        }
        "computer_type" => {
            let Some(t) = text(input, "text") else { return Out::err("computer_type needs text".to_owned()) };
            acted(live, hands(Input::Type { text: t.to_owned() }), since).await
        }
        "computer_key" => {
            let Some(k) = text(input, "key") else { return Out::err("computer_key needs key".to_owned()) };
            acted(live, hands(Input::Key { key: k.to_owned() }), since).await
        }
        "computer_scroll" => {
            let (Some(x), Some(y)) = (num(input, "x"), num(input, "y")) else { return Out::err("computer_scroll needs x and y".to_owned()) };
            let (dx, dy) = (num(input, "dx").unwrap_or(0), num(input, "dy").unwrap_or(0));
            acted(live, hands(Input::Scroll { x, y, dx, dy }), since).await
        }
        "computer_run" => {
            let Some(command) = text(input, "command") else { return Out::err("computer_run needs command".to_owned()) };
            // on a shared computer a command could reach the other Bots' screens, until the box
            // keeps each screen's commands to its own user
            if Computers::seats(&run.ctx, &live.computer).len() > 1 && !provider.commands_stay_on_their_screen(machine).await {
                return Out::err("computer_run is off on a computer several Bots share: a command could reach their screens. Use the screen, or ask for a computer of your own.".to_owned());
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
            acted(live, provider.open_url(machine, screen, url), since).await
        }
        "computer_wait" => {
            let idle_ms = input.get("idle_ms").and_then(Value::as_u64).unwrap_or(300).clamp(50, 10_000);
            let timeout_ms = input.get("timeout_ms").and_then(Value::as_u64).unwrap_or(8000).clamp(100, 60_000);
            match provider.settle(machine, screen, WaitFor { idle_ms, timeout_ms, since: None }).await {
                Ok(s) => look(live, json!({"changed": s.changed, "waited_ms": s.waited_ms, "timed_out": s.timed_out})).await,
                Err(p) => no(&p),
            }
        }
        "computer_read_file" => {
            let Some(path) = text(input, "path").and_then(|p| own_path(&screen.folder, p)) else { return Out::err("computer_read_file reads inside your own folder on the computer".to_owned()) };
            match provider.read_file(machine, &path).await {
                Ok(bytes) => Out::ok(machine.scrub(&js::slice_utf16(&String::from_utf8_lossy(&bytes), MAX_TEXT))),
                Err(p) => no(&p),
            }
        }
        "computer_write_file" => {
            let Some(path) = text(input, "path").and_then(|p| own_path(&screen.folder, p)) else { return Out::err("computer_write_file writes inside your own folder on the computer".to_owned()) };
            let content = text(input, "content").unwrap_or_default();
            match provider.write_file(machine, &path, content.as_bytes()).await {
                Ok(()) => Out::ok(js::stringify(&json!({"saved": path, "bytes": content.len()}))),
                Err(p) => no(&p),
            }
        }
        "computer_record" => match text(input, "action") {
            Some("start") => match provider.record_start(machine, screen).await {
                Ok(id) => Out::ok(js::stringify(&json!({"recording": id}))),
                Err(p) => no(&p),
            },
            Some("stop") => {
                let Some(id) = input.get("id").and_then(Value::as_u64) else { return Out::err("computer_record stop needs the id start gave".to_owned()) };
                match provider.record_stop(machine, screen, id).await {
                    Ok(path) => Out::ok(js::stringify(&json!({"saved": path}))),
                    Err(p) => no(&p),
                }
            }
            _ => Out::err("computer_record's action is start or stop".to_owned()),
        },
        _ => Out::err(format!("unknown tool {name}")),
    }
}
