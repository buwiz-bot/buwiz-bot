//! `/messages`: which Bots can message this one. Built on the composer's
//! `/` list, not the plugin resolve-and-ticket protocol: rows come from the
//! Bots the app already holds, nothing here reaches the model, and a pick
//! only fills the box, so Enter on a complete line is what changes anything.
//!
//! - `/messages`, `/messages nova`, ⌘K's "Messages…": the card, a `messages`
//!   notice the broker posts in the open chat. It carries only whose card it
//!   is: what it draws is read now, so one in the scrollback is never wrong.
//! - `/messages allow all`, `/messages allow scout`, `/messages none`: a
//!   change to the open chat's Bot through `/v1/host/agents/accepts`.
//!
//! Only the person's own Enter runs these: `store::send_text` (a chip, a
//! choice card) sends the same words on to the AI as plain text, and a
//! plugin's `prompt` answer can't place them in the box.

use nsh_core::prelude::*;
use serde_json::{Value, json};

use crate::agents::{Accepts, listed_bot, name_of};
use crate::live::store;
use crate::slash::{Pick, PopRow};
use crate::store::RoxBot;
use crate::theme;

const OPEN_A_BOT: &str = "Open a Bot's chat first.";
const HELPER: &str = "A helper takes messages only from its parent.";
/// No answer: the broker may have applied it all the same.
const UNCONFIRMED: &str = "Couldn't confirm that change. Showing what the broker has now.";
/// What pressing Some leaves in the box: the list opens for the first name.
const ALLOW: &str = "/messages allow ";

/// Whether `text`, trimmed, is `/messages`: `dispatch`'s reading of a
/// built-in's name.
pub(crate) fn is_command(text: &str) -> bool {
    text.strip_prefix("/messages").is_some_and(|r| r.is_empty() || r.starts_with(|c: char| c == ':' || c.is_whitespace()))
}

/// The chat a command was typed in.
enum Here {
    Plain,
    Helper(Box<RoxBot>),
    Bot(Box<RoxBot>),
}

fn here() -> Here {
    match store().active_bot_untracked() {
        None => Here::Plain,
        Some(b) if b.helper.is_some() => Here::Helper(Box::new(b)),
        Some(b) => Here::Bot(Box::new(b)),
    }
}

/// What follows `/messages`, read whole.
#[derive(PartialEq, Debug)]
enum Line<'a> {
    Open,
    Named(&'a str),
    AllowAll,
    Allow(&'a str),
    None,
    /// `all` alone or `allow none`: these words are no Bot's name, and no
    /// change either. What to say.
    NotACommand(&'static str),
}

/// `allow <tail>`: the tail, which may be empty.
fn after_allow(line: &str) -> Option<&str> {
    let (word, tail) = line.split_once(char::is_whitespace)?;
    word.eq_ignore_ascii_case("allow").then_some(tail.trim_start())
}

fn parse(rest: &str) -> Line<'_> {
    let line = rest.trim();
    let is = |w: &str, word: &str| w.eq_ignore_ascii_case(word);
    if line.is_empty() {
        Line::Open
    } else if is(line, "none") {
        Line::None
    } else if is(line, "all") {
        Line::NotACommand("To let every Bot in: /messages allow all")
    } else {
        match after_allow(line) {
            Some(who) if is(who, "all") => Line::AllowAll,
            Some(who) if is(who, "none") => Line::NotACommand("To hear from nobody: /messages none"),
            Some(who) => Line::Allow(who),
            None => Line::Named(line),
        }
    }
}

/// What a line comes to.
#[derive(PartialEq, Debug)]
enum Outcome {
    Say(String),
    Card(String),
    Set { bot: String, accepts: Accepts },
}

/// The one Bot called `who`, ignoring case; never a helper. Why not, else.
fn named<'a>(who: &str, bots: &'a [RoxBot]) -> Result<&'a RoxBot, String> {
    let who_l = who.to_lowercase();
    let mut found = bots.iter().filter(|b| listed_bot(b) && b.name.trim().to_lowercase() == who_l);
    match (found.next(), found.next()) {
        (Some(b), None) => Ok(b),
        (Some(_), Some(_)) => Err(format!("More than one Bot is called {who}.")),
        _ => Err(format!("No Bot is called {who}.")),
    }
}

/// `who` added to `b`'s list, or why that changes nothing.
fn allowing(b: &RoxBot, who: &str, bots: &[RoxBot]) -> Outcome {
    let other = match named(who, bots) {
        Ok(o) => o,
        Err(why) => return Outcome::Say(why),
    };
    if other.id == b.id {
        return Outcome::Say(format!("{} doesn't need to allow itself.", b.name));
    }
    let already = Outcome::Say(format!("{} can already message {}.", other.name, b.name));
    let mut list = match &b.accepts {
        Accepts::All => return already,
        Accepts::List(l) if l.contains(&other.id) => return already,
        Accepts::List(l) => l.clone(),
        Accepts::None => Vec::new(),
    };
    list.push(other.id.clone());
    Outcome::Set { bot: b.id.clone(), accepts: Accepts::List(list) }
}

fn outcome(line: &Line, here: &Here, bots: &[RoxBot]) -> Outcome {
    let say = |s: &str| Outcome::Say(s.to_owned());
    match (line, here) {
        (Line::NotACommand(s), _) => say(s),
        (Line::Open, Here::Plain) => say("Open a Bot's chat, or name one: /messages <Bot>"),
        (Line::Open, Here::Helper(b) | Here::Bot(b)) => Outcome::Card(b.id.clone()),
        (Line::Named(who), _) => match named(who, bots) {
            Ok(b) => Outcome::Card(b.id.clone()),
            Err(_) if who.eq_ignore_ascii_case("allow") => say("Allow whom? /messages allow <Bot>"),
            Err(why) => Outcome::Say(why),
        },
        (_, Here::Plain) => say(OPEN_A_BOT),
        (_, Here::Helper(_)) => say(HELPER),
        (Line::None, Here::Bot(b)) => Outcome::Set { bot: b.id.clone(), accepts: Accepts::None },
        (Line::AllowAll, Here::Bot(b)) => Outcome::Set { bot: b.id.clone(), accepts: Accepts::All },
        (Line::Allow(who), Here::Bot(b)) => allowing(b, who, bots),
    }
}

/// A row that fills the box with `fill`.
fn row(fill: String, label: &str, detail: String, bot: &str) -> PopRow {
    PopRow { pick: Pick::Fill(fill), label: label.to_owned(), hint: String::new(), detail, bot: bot.to_owned() }
}

/// The rows and header for `rest`, what follows `/messages `. Picking a row
/// must run what it says, so a Bot whose name reads as a command is no row
/// (`all` and `none` win over the name).
fn rows(rest: &str, here: &Here, bots: &[RoxBot]) -> (Vec<PopRow>, String) {
    let line = rest.trim_start();
    let starts = |word: &str, q: &str| word.to_lowercase().starts_with(&q.to_lowercase());
    let listed = || bots.iter().filter(|b| listed_bot(b));
    let Some(q) = after_allow(line) else {
        let mut out = Vec::new();
        if starts("allow", line) {
            out.push(row("/messages allow ".to_owned(), "allow", "Let a Bot, or every Bot, message this one".to_owned(), ""));
        }
        if starts("none", line) {
            out.push(row("/messages none".to_owned(), "none", "Hear from no Bot".to_owned(), ""));
        }
        out.extend(listed().filter(|b| starts(&b.name, line) && matches!(parse(&b.name), Line::Named(_))).map(|b| {
            row(format!("/messages {}", b.name), &b.name, format!("Who can message {}", b.name), &b.id)
        }));
        return (out, String::new());
    };
    let this = match here {
        Here::Bot(b) => Some(b),
        _ => None,
    };
    let can_write = |b: &RoxBot| match this.map(|t| &t.accepts) {
        Some(Accepts::All) => true,
        Some(Accepts::List(l)) => l.contains(&b.id),
        _ => false,
    };
    let mut out = Vec::new();
    if starts("all", q) {
        out.push(row("/messages allow all".to_owned(), "all", "Let every Bot message this one".to_owned(), ""));
    }
    out.extend(
        listed()
            .filter(|b| this.is_none_or(|t| t.id != b.id) && !can_write(b) && starts(&b.name, q))
            .filter(|b| matches!(parse(&format!("allow {}", b.name)), Line::Allow(_)))
            .map(|b| row(format!("/messages allow {}", b.name), &b.name, "Let it message this one".to_owned(), &b.id)),
    );
    let head = match here {
        Here::Plain => OPEN_A_BOT.to_owned(),
        Here::Helper(_) => HELPER.to_owned(),
        Here::Bot(b) if b.accepts == Accepts::All => format!("Every Bot can already message {}.", b.name),
        Here::Bot(_) => String::new(),
    };
    (out, head)
}

/// [`crate::slash`]'s `menu`: the list past `/messages `.
pub(crate) fn menu(rest: &str) -> (Vec<PopRow>, String) {
    rows(rest, &here(), &store().bots.get_untracked())
}

/// [`crate::slash`]'s `run`: `rest` is what follows `/messages`.
pub(crate) fn run(rest: &str) {
    run_as(rest, format!("/messages{rest}"));
}

/// ⌘K's "Messages…": `/messages` with no line typed.
pub(crate) fn open_card() {
    run_as("", String::new());
}

fn run_as(rest: &str, said: String) {
    let st = store();
    match outcome(&parse(rest), &here(), &st.bots.get_untracked()) {
        Outcome::Say(s) => crate::toast::show(s),
        Outcome::Card(bot) => post_card(bot, said),
        Outcome::Set { bot, accepts } => {
            let (id, chat) = (bot.clone(), st.active_id.get_untracked());
            st.agents_accepts(bot, accepts, move |r| {
                answered(&id, &chat, r);
            });
        }
    }
}

/// Asks the broker for `bot`'s card in the open chat, `said` the line typed.
fn post_card(bot: String, said: String) {
    let st = store();
    let chat = st.active_id.get_untracked();
    if chat.is_empty() {
        return crate::toast::show("Open a chat first.");
    }
    crate::store::spawn_detached(async move {
        match crate::client::post_any("/v1/host/agents/card", json!({"chat": chat, "bot": bot, "said": said.trim()})).await {
            Some(v) if v.get("ok").and_then(Value::as_bool) == Some(true) => store().refresh_turns(),
            Some(v) => crate::toast::show(crate::tools::refusal(&v, "")),
            None => crate::toast::show("Couldn't reach the broker."),
        }
    });
}

/// The route's answer to a change made in `chat`: `{ok, text, undo}` is the
/// receipt there. With another chat open by then it is a toast, the Undo
/// staying in `/tools`; a refusal is a toast. True when it went through.
fn answered(bot: &str, chat: &str, r: Option<Value>) -> bool {
    let st = store();
    match r {
        Some(v) if v.get("ok").and_then(Value::as_bool) == Some(true) => {
            if st.active_id.get_untracked() == chat {
                let name = st.bots.with_untracked(|b| name_of(bot, b));
                st.add_local_wire_turn(&receipt(bot, &name, &v, crate::tools::now_ms()));
            } else {
                crate::toast::show(away(&v));
            }
            return true;
        }
        Some(v) => crate::toast::show(crate::tools::refusal(&v, "")),
        None => crate::toast::show(UNCONFIRMED),
    }
    false
}

/// What a change says with its chat no longer open.
fn away(answer: &Value) -> String {
    let text = answer.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()).unwrap_or("Changed.");
    if answer.get("undo").and_then(Value::as_str).is_some_and(|u| !u.is_empty()) { format!("{text} Undo it in /tools.") } else { text.to_owned() }
}

/// The route's answer as an applied `tools_change` turn, the receipt a
/// change gets in the chat: the route posts none, and it lasts 8 s, so it
/// stays here. `undo` null or missing (nothing changed, an older broker):
/// no Undo.
fn receipt(bot: &str, name: &str, answer: &Value, at: i64) -> Value {
    let text = answer.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()).unwrap_or("Changed.");
    json!({"role": "notice", "text": text, "notice": {
        "kind": "tools_change", "id": format!("p_messages_{at}"), "bot": bot, "botName": name, "op": "accepts",
        "state": "applied", "at": at, "undo": answer.get("undo"), "text": text,
    }})
}

/// A `messages` notice: whose card it is, its name when posted (for a Bot
/// since gone) and the line that asked for it.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct Notice {
    pub bot: String,
    pub bot_name: String,
    pub said: String,
}

impl Notice {
    pub(crate) fn of(v: &Value) -> Option<Self> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        (s("kind") == "messages" && !s("bot").is_empty()).then(|| Self { bot: s("bot"), bot_name: s("botName"), said: s("said") })
    }
}

/// One choice of a control: its word, whether it holds now, and its press.
type Choice = (&'static str, bool, Box<dyn Fn()>);

/// A caption over its choices, each button named "Hears: All".
fn control(caption: &str, choices: Vec<Choice>) -> impl IntoElement {
    let mut seg = view().flex_row().gap(px(2.0)).p(px(2.0)).rounded(px(8.0)).bg(theme::HOVER);
    for (word, picked, on) in choices {
        let hov = signal(false);
        seg = seg.child(
            view()
                .label(crate::tools::position_name(caption, word))
                .px(px(9.0))
                .py(px(3.0))
                .rounded(px(6.0))
                .bg(move || if picked { theme::MAIN } else if hov.get() { theme::ACTIVE } else { AdaptiveColor::TRANSPARENT })
                .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
                .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
                .on_press(move || {
                    if !picked {
                        on()
                    }
                })
                .child(text(word).text_size(px(12.0)).text_color(if picked { theme::TEXT } else { theme::MUTED })),
        );
    }
    view().label(caption.to_owned()).gap(px(3.0)).child(text(caption.to_owned()).text_size(px(11.0)).text_color(theme::MUTED)).child(seg)
}

/// Whom `id` hears from changes, with its receipt and Undo.
fn change(id: &str, to: Accepts) {
    let (bot, chat) = (id.to_owned(), store().active_id.get_untracked());
    store().agents_accepts(id.to_owned(), to, move |r| {
        answered(&bot, &chat, r);
    });
}

/// Some: an empty list, said with Undo, then the box ready for the first
/// name. Only in that Bot's own chat, where `allow` means it, and never
/// over words the person is writing.
fn some(id: &str) {
    let (bot, chat) = (id.to_owned(), store().active_id.get_untracked());
    store().agents_accepts(id.to_owned(), Accepts::List(Vec::new()), move |r| {
        let st = store();
        let here = st.active_bot_untracked().is_some_and(|b| b.id == bot);
        if answered(&bot, &chat, r) && here && st.draft.get_untracked().trim().is_empty() {
            crate::composer::draft_changed(ALLOW);
            crate::composer::focus();
        }
    });
}

/// Hears: All, Some or None, as it is now.
fn hears_control(b: &RoxBot) -> impl IntoElement {
    let (id1, id2, id3) = (b.id.clone(), b.id.clone(), b.id.clone());
    control(
        "Hears",
        vec![
            ("All", b.accepts == Accepts::All, Box::new(move || change(&id1, Accepts::All))),
            ("Some", matches!(b.accepts, Accepts::List(_)), Box::new(move || some(&id2))),
            ("None", b.accepts == Accepts::None, Box::new(move || change(&id3, Accepts::None))),
        ],
    )
}

/// Sends: the `agents` switch, as `/tools` sets it.
fn sends_control(id: &str) -> impl IntoElement {
    let now = crate::tools::sends_of(id);
    let choice = |word: &'static str, state: &'static str| -> Choice {
        let bot = id.to_owned();
        (
            word,
            now == state,
            Box::new(move || {
                let (b, chat) = (bot.clone(), store().active_id.get_untracked());
                crate::tools::set_sends(bot.clone(), state, move |r| {
                    answered(&b, &chat, r);
                });
            }),
        )
    };
    control("Can it start a message", vec![choice("On", "on"), choice("Ask", "ask"), choice("Off", "off")])
}

/// A Bot on the list, with its ✕: off at once, with Undo.
fn allowed_row(b: &RoxBot, who: &str, bots: &[RoxBot], list: &[String]) -> impl IntoElement {
    let name = name_of(who, bots);
    let (id, left) = (b.id.clone(), list.iter().filter(|x| x.as_str() != who).cloned().collect::<Vec<_>>());
    view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .pl(px(8.0))
        .pr(px(4.0))
        .py(px(2.0))
        .rounded(px(8.0))
        .bg(theme::HOVER)
        .child(crate::chat::peer_avatar(who, &name, 16.0))
        .child(text(name.clone()).text_size(px(12.0)).text_color(theme::TEXT).number_of_lines(1))
        .child(
            view()
                .label(format!("Take {name} off the list"))
                .size(px(18.0))
                .items_center()
                .justify_center()
                .on_press(move || change(&id, Accepts::List(left.clone())))
                .child(text("✕").text_size(px(10.0)).text_color(theme::MUTED)),
        )
}

/// The card's Bot, and its controls beside or under it.
pub(crate) const WHO: &str = "Whose card";
pub(crate) const CONTROLS: &str = "Message settings";

/// The card as things are now (tracked: Hears from the Bots held here,
/// Sends as last read).
fn card(n: &Notice) -> AnyElement {
    let bots = store().bots.get();
    let Some(b) = bots.iter().find(|b| b.id == n.bot).cloned() else {
        return text(format!("{} is gone.", n.bot_name)).text_size(px(12.0)).text_color(theme::MUTED).into_element();
    };
    let title = format!("Who can message {}?", b.name);
    // One line while it all fits. Narrower, the controls go under the name
    // as one block, and narrower still they stack. The name keeps its own
    // width: a zero basis (`flex_1`) would count it as nothing when the row
    // wraps, and leave it under the controls.
    let who = view()
        .label(WHO)
        .flex_row()
        .items_center()
        .gap_2()
        .flex_grow(1.0)
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .child(crate::chat::peer_avatar(&b.id, &b.name, 28.0))
        .child(text(b.name.clone()).text_size(px(13.0)).text_color(theme::TEXT).number_of_lines(1).flex_shrink(1.0).min_w(px(0.0)));
    let controls = view()
        .label(CONTROLS)
        .flex_row()
        .flex_wrap()
        .gap_3()
        .flex_shrink(1.0)
        .when(b.helper.is_none(), |v| v.child(hears_control(&b)))
        .child(sends_control(&b.id));
    let row = view().flex_row().flex_wrap().items_center().gap_3().child(who).child(controls);
    let mut c = crate::cards::shell(&format!("messages-card-{}", b.id))
        .label(title.clone())
        .child(text(title).text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT))
        .child(row);
    if b.helper.is_some() {
        c = c.child(text(HELPER).text_size(px(12.0)).text_color(theme::MUTED));
    } else if let Accepts::List(list) = &b.accepts {
        c = if list.is_empty() {
            c.child(text("No Bot is on the list yet.").text_size(px(12.0)).text_color(theme::MUTED))
        } else {
            c.child(view().flex_row().flex_wrap().gap(px(6.0)).children(list.iter().map(|who| allowed_row(&b, who, &bots, list))))
        };
    }
    c.into_element()
}

/// The `messages` notice's row: the line that asked for it, then the card.
pub(crate) fn card_row(n: Notice) -> impl IntoElement {
    let bot = n.bot.clone();
    // a Bot's own switch, or a card answered in this chat, moves Sends too
    effect(move || {
        store().turns.with(|t| t.len());
        crate::tools::sends_watch();
        crate::tools::load_sends(bot.clone());
    });
    let said = n.said.clone();
    view()
        .w_full()
        .gap(px(6.0))
        .when(!said.is_empty(), |v| v.child(view().flex_row().justify_end().child(text(said).text_size(px(13.0)).text_color(theme::MUTED))))
        .child(view().flex_row().w_full().justify_center().child(dynamic(move || card(&n))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::HelperOf;
    use crate::store::Turn;

    fn bot(id: &str, name: &str, accepts: Accepts) -> RoxBot {
        RoxBot { id: id.to_owned(), name: name.to_owned(), accepts, ..Default::default() }
    }

    /// Nova, Scout, Ada, and Ada's helper.
    fn bots() -> Vec<RoxBot> {
        let helper = RoxBot { helper: Some(HelperOf { parent: "ada".to_owned(), ..Default::default() }), ..bot("h1", "Scribe", Accepts::All) };
        vec![bot("nova", "Nova", Accepts::All), bot("scout", "Scout", Accepts::All), bot("ada", "Ada", Accepts::All), helper]
    }

    fn at(accepts: Accepts) -> Here {
        Here::Bot(Box::new(bot("nova", "Nova", accepts)))
    }

    fn labels(rest: &str, here: &Here, bots: &[RoxBot]) -> Vec<String> {
        rows(rest, here, bots).0.into_iter().map(|r| r.label).collect()
    }

    fn fills(rest: &str, here: &Here, bots: &[RoxBot]) -> Vec<String> {
        rows(rest, here, bots)
            .0
            .into_iter()
            .map(|r| match r.pick {
                Pick::Fill(t) => t,
                _ => panic!("every row fills"),
            })
            .collect()
    }

    fn run_line(rest: &str, here: &Here) -> Outcome {
        outcome(&parse(rest), here, &bots())
    }

    fn says(s: &str) -> Outcome {
        Outcome::Say(s.to_owned())
    }

    #[test]
    fn past_the_name_the_list_is_allow_none_then_the_bots() {
        let (b, h) = (bots(), at(Accepts::All));
        assert_eq!(labels("", &h, &b), ["allow", "none", "Nova", "Scout", "Ada"], "no helper");
        assert_eq!(fills("", &h, &b), ["/messages allow ", "/messages none", "/messages Nova", "/messages Scout", "/messages Ada"]);
        assert_eq!(labels("n", &h, &b), ["none", "Nova"]);
        assert_eq!(labels("N", &h, &b), ["none", "Nova"], "ignoring case");
        assert_eq!(labels("a", &h, &b), ["allow", "Ada"]);
        assert_eq!(labels("zz", &h, &b), Vec::<String>::new());
        let rs = rows("sc", &h, &b).0;
        assert_eq!((rs[0].bot.as_str(), rows("n", &h, &b).0[0].bot.as_str()), ("scout", ""), "a Bot's row carries it for its avatar");
    }

    #[test]
    fn past_allow_the_list_is_all_then_the_bots_not_yet_allowed() {
        let b = bots();
        // nobody yet: everyone but this Bot and helpers
        let h = at(Accepts::None);
        assert_eq!(labels("allow ", &h, &b), ["all", "Scout", "Ada"]);
        assert_eq!(fills("allow ", &h, &b), ["/messages allow all", "/messages allow Scout", "/messages allow Ada"]);
        assert_eq!(labels("allow s", &h, &b), ["Scout"]);
        assert_eq!(labels("  ALLOW   a", &h, &b), ["all", "Ada"], "spaces and case");
        // already on the list: gone
        let h = at(Accepts::List(vec!["scout".to_owned()]));
        assert_eq!(labels("allow ", &h, &b), ["all", "Ada"]);
        // at All they are all in: the header says so
        let h = at(Accepts::All);
        assert_eq!(labels("allow ", &h, &b), ["all"]);
        assert_eq!(rows("allow ", &h, &b).1, "Every Bot can already message Nova.");
    }

    #[test]
    fn a_name_narrows_across_its_spaces() {
        let b = vec![bot("nova", "Nova", Accepts::None), bot("t", "Tax Helper", Accepts::All)];
        let h = Here::Bot(Box::new(b[0].clone()));
        assert_eq!(labels("allow tax h", &h, &b), ["Tax Helper"]);
        assert_eq!(labels("tax h", &h, &b), ["Tax Helper"]);
        assert_eq!(run_on("allow tax helper", &h, &b), Outcome::Set { bot: "nova".to_owned(), accepts: Accepts::List(vec!["t".to_owned()]) });
    }

    fn run_on(rest: &str, here: &Here, b: &[RoxBot]) -> Outcome {
        outcome(&parse(rest), here, b)
    }

    /// A row names a Bot only if picking it runs that: a Bot named like a
    /// word of the command would otherwise change who gets in.
    #[test]
    fn all_and_none_win_over_a_bot_of_that_name() {
        let b = vec![bot("nova", "Nova", Accepts::None), bot("x1", "All", Accepts::All), bot("x2", "NONE", Accepts::All), bot("x3", "Allow Bob", Accepts::All), bot("x4", "Bob", Accepts::All)];
        let h = Here::Bot(Box::new(b[0].clone()));
        assert_eq!(run_on("none", &h, &b), Outcome::Set { bot: "nova".to_owned(), accepts: Accepts::None });
        assert_eq!(run_on("allow ALL", &h, &b), Outcome::Set { bot: "nova".to_owned(), accepts: Accepts::All });
        assert_eq!(run_on("all", &h, &b), says("To let every Bot in: /messages allow all"));
        assert_eq!(run_on("allow none", &h, &b), says("To hear from nobody: /messages none"));
        assert_eq!(labels("", &h, &b), ["allow", "none", "Nova", "Bob"]);
        assert_eq!(labels("allow ", &h, &b), ["all", "Allow Bob", "Bob"]);
    }

    #[test]
    fn changes_mean_the_open_bot() {
        let set = |accepts| Outcome::Set { bot: "nova".to_owned(), accepts };
        assert_eq!(run_line("none", &at(Accepts::All)), set(Accepts::None));
        assert_eq!(run_line(" NONE ", &at(Accepts::All)), set(Accepts::None));
        assert_eq!(run_line("allow all", &at(Accepts::None)), set(Accepts::All));
        assert_eq!(run_line("allow scout", &at(Accepts::None)), set(Accepts::List(vec!["scout".to_owned()])));
        let on = Accepts::List(vec!["ada".to_owned()]);
        assert_eq!(run_line("Allow SCOUT", &at(on)), set(Accepts::List(vec!["ada".to_owned(), "scout".to_owned()])), "added to the list, not over it");
    }

    #[test]
    fn allowing_who_is_in_already_changes_nothing() {
        assert_eq!(run_line("allow scout", &at(Accepts::All)), says("Scout can already message Nova."));
        assert_eq!(run_line("allow scout", &at(Accepts::List(vec!["scout".to_owned()]))), says("Scout can already message Nova."));
    }

    #[test]
    fn a_change_can_only_name_one_real_other_bot() {
        let h = at(Accepts::None);
        assert_eq!(run_line("allow nova", &h), says("Nova doesn't need to allow itself."));
        assert_eq!(run_line("allow scribe", &h), says("No Bot is called scribe."), "a helper is no choice");
        assert_eq!(run_line("allow sc", &h), says("No Bot is called sc."), "the whole name, not a part");
        let b = vec![bot("nova", "Nova", Accepts::None), bot("s1", "Scout", Accepts::All), bot("s2", "scout", Accepts::All)];
        assert_eq!(run_on("allow Scout", &Here::Bot(Box::new(b[0].clone())), &b), says("More than one Bot is called Scout."));
    }

    #[test]
    fn plain_and_helper_chats_refuse_a_change_but_can_read() {
        for line in ["none", "allow all", "allow scout"] {
            assert_eq!(run_line(line, &Here::Plain), says("Open a Bot's chat first."), "{line}");
            assert_eq!(run_line(line, &Here::Helper(Box::new(bot("h1", "Scribe", Accepts::All)))), says("A helper takes messages only from its parent."), "{line}");
        }
        let helper = || Here::Helper(Box::new(bot("h1", "Scribe", Accepts::All)));
        assert_eq!(run_line("", &helper()), Outcome::Card("h1".to_owned()), "its own card, which shows no Hears");
        assert_eq!(run_line("", &Here::Plain), says("Open a Bot's chat, or name one: /messages <Bot>"));
        assert_eq!(run_line("scout", &Here::Plain), Outcome::Card("scout".to_owned()));
        assert_eq!(rows("allow ", &Here::Plain, &bots()).1, "Open a Bot's chat first.");
        assert_eq!(rows("allow ", &Here::Helper(Box::new(bot("h1", "Scribe", Accepts::All))), &bots()).1, "A helper takes messages only from its parent.");
    }

    #[test]
    fn reading_a_bot_is_its_card_and_never_a_change() {
        let h = at(Accepts::None);
        assert_eq!(run_line("", &h), Outcome::Card("nova".to_owned()));
        assert_eq!(run_line("SCOUT", &h), Outcome::Card("scout".to_owned()), "this chat, another Bot's card");
        assert_eq!(run_line("allow", &h), says("Allow whom? /messages allow <Bot>"));
    }

    #[test]
    fn a_notice_is_only_whose_card_it_is() {
        let n = Notice::of(&json!({"kind": "messages", "bot": "nova", "botName": "Nova", "said": "/messages"})).expect("a card");
        assert_eq!((n.bot.as_str(), n.bot_name.as_str(), n.said.as_str()), ("nova", "Nova", "/messages"));
        assert!(Notice::of(&json!({"kind": "messages"})).is_none(), "no Bot, no card");
        assert!(Notice::of(&json!({"kind": "tools_change", "bot": "nova"})).is_none());
        let t = Turn::of(&json!({"role": "notice", "text": "Who can message Nova?", "notice": {"kind": "messages", "bot": "nova"}}));
        assert!(t.messages_card.is_some() && t.tools_change.is_none());
        assert!(Turn::of(&json!({"role": "user", "notice": {"kind": "messages", "bot": "nova"}})).messages_card.is_none());
    }

    #[test]
    fn only_the_name_itself_is_the_command() {
        for t in ["/messages", "/messages none", "/messages\tnone", "/messages:none"] {
            assert!(is_command(t), "{t}");
        }
        for t in ["/messagesx", "/message", "messages", "see /messages none", "/short:messages"] {
            assert!(!is_command(t), "{t}");
        }
    }

    fn notice(answer: Value) -> crate::store::ToolsChangeNotice {
        Turn::of(&receipt("nova", "Nova", &answer, 5)).tools_change.expect("a tools_change notice")
    }

    #[test]
    fn with_its_chat_gone_a_change_says_where_undo_is_while_it_has_one() {
        assert_eq!(away(&json!({"ok": true, "text": "Nova hears from nobody now.", "undo": "u_1"})), "Nova hears from nobody now. Undo it in /tools.");
        assert_eq!(away(&json!({"ok": true, "text": "x", "undo": null})), "x");
        assert_eq!(away(&json!({"ok": true})), "Changed.");
    }

    #[test]
    fn the_answer_is_an_applied_receipt_with_undo_while_it_has_one() {
        let n = notice(json!({"ok": true, "text": "Scout can now message Nova.", "undo": "u_1"}));
        assert_eq!((n.state.as_str(), n.receipt.as_str(), n.undo.as_str(), n.bot.as_str(), n.at), ("applied", "Scout can now message Nova.", "u_1", "nova", 5));
        let shows = |n| crate::tools::shows(&n, None, 5, 6);
        assert_eq!(shows(n), crate::tools::Shows::Receipt { text: "Scout can now message Nova.".to_owned(), undo: "u_1".to_owned(), at: 5 });
        for answer in [json!({"ok": true, "text": "x", "undo": null}), json!({"ok": true, "text": "x"})] {
            assert_eq!(shows(notice(answer)), crate::tools::Shows::Receipt { text: "x".to_owned(), undo: String::new(), at: 5 }, "no Undo button");
        }
        assert_eq!(notice(json!({"ok": true})).receipt, "Changed.", "an older broker's bare answer");
    }
}
