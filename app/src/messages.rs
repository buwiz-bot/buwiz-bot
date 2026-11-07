//! `/messages`: which Bots can message this one. Built on the composer's
//! `/` list, not the plugin resolve-and-ticket protocol: rows come from the
//! Bots the app already holds, nothing here reaches the model, and a pick
//! only fills the box, so Enter on a complete line is what changes anything.
//!
//! - `/messages`, `/messages nova`: whom a Bot hears now (plain words until
//!   the card comes).
//! - `/messages allow all`, `/messages allow scout`, `/messages none`: a
//!   change to the open chat's Bot through `/v1/host/agents/accepts`.
//!
//! Only the person's own Enter runs these: `store::send_text` (a chip, a
//! choice card) sends the same words on to the AI as plain text.

use serde_json::{Value, json};

use crate::agents::{Accepts, listed_bot, name_of};
use crate::live::store;
use crate::slash::{Pick, PopRow};
use crate::store::RoxBot;

const OPEN_A_BOT: &str = "Open a Bot's chat first.";
const HELPER: &str = "A helper takes messages only from its parent.";
const NOT_CONNECTED: &str = "Couldn't reach the broker, so nothing changed.";

/// Whether `text`, trimmed, is `/messages`: `dispatch`'s reading of a
/// built-in's name.
pub(crate) fn is_command(text: &str) -> bool {
    text.strip_prefix("/messages").is_some_and(|r| r.is_empty() || r.starts_with(|c: char| c == ':' || c.is_whitespace()))
}

/// The chat a command was typed in.
enum Here {
    Plain,
    Helper,
    Bot(Box<RoxBot>),
}

fn here() -> Here {
    match store().active_bot_untracked() {
        None => Here::Plain,
        Some(b) if b.helper.is_some() => Here::Helper,
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

/// Whom `b` hears from, in words.
fn hears(b: &RoxBot, bots: &[RoxBot]) -> String {
    match &b.accepts {
        Accepts::All => format!("Every Bot can message {}.", b.name),
        Accepts::None => format!("{} hears from nobody.", b.name),
        Accepts::List(l) if l.is_empty() => format!("No Bot is on {}'s list.", b.name),
        Accepts::List(l) => format!("{} hears only from {}.", b.name, l.iter().map(|id| name_of(id, bots)).collect::<Vec<_>>().join(", ")),
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
        (Line::Open, Here::Helper) => say(HELPER),
        (Line::Open, Here::Bot(b)) => Outcome::Say(hears(b, bots)),
        (Line::Named(who), _) => match named(who, bots) {
            Ok(b) => Outcome::Say(hears(b, bots)),
            Err(_) if who.eq_ignore_ascii_case("allow") => say("Allow whom? /messages allow <Bot>"),
            Err(why) => Outcome::Say(why),
        },
        (_, Here::Plain) => say(OPEN_A_BOT),
        (_, Here::Helper) => say(HELPER),
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
        Here::Helper => HELPER.to_owned(),
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
    let st = store();
    match outcome(&parse(rest), &here(), &st.bots.get_untracked()) {
        Outcome::Say(s) => crate::toast::show(s),
        Outcome::Set { bot, accepts } => {
            let id = bot.clone();
            st.agents_accepts(bot, accepts, move |r| answered(&id, r));
        }
    }
}

/// The route's answer: `{ok, text, undo}` is the receipt, a refusal a toast.
fn answered(bot: &str, r: Option<Value>) {
    let st = store();
    match r {
        Some(v) if v.get("ok").and_then(Value::as_bool) == Some(true) => {
            let name = st.bots.with_untracked(|b| name_of(bot, b));
            st.add_local_wire_turn(&receipt(bot, &name, &v, crate::tools::now_ms()));
        }
        Some(v) => crate::toast::show(v.get("message").and_then(Value::as_str).filter(|m| !m.is_empty()).unwrap_or(NOT_CONNECTED).to_owned()),
        None => crate::toast::show(NOT_CONNECTED),
    }
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
            assert_eq!(run_line(line, &Here::Helper), says("A helper takes messages only from its parent."), "{line}");
        }
        assert_eq!(run_line("", &Here::Helper), says("A helper takes messages only from its parent."));
        assert_eq!(run_line("", &Here::Plain), says("Open a Bot's chat, or name one: /messages <Bot>"));
        assert_eq!(run_line("scout", &Here::Plain), says("Every Bot can message Scout."));
        assert_eq!(rows("allow ", &Here::Plain, &bots()).1, "Open a Bot's chat first.");
        assert_eq!(rows("allow ", &Here::Helper, &bots()).1, "A helper takes messages only from its parent.");
    }

    #[test]
    fn reading_a_bot_says_whom_it_hears_and_never_changes_it() {
        let b = vec![bot("nova", "Nova", Accepts::List(vec!["scout".to_owned(), "ada".to_owned()])), bot("scout", "Scout", Accepts::None), bot("ada", "Ada", Accepts::List(Vec::new()))];
        assert_eq!(run_on("", &Here::Bot(Box::new(b[0].clone())), &b), says("Nova hears only from Scout, Ada."));
        assert_eq!(run_on("SCOUT", &Here::Bot(Box::new(b[0].clone())), &b), says("Scout hears from nobody."));
        assert_eq!(run_on("ada", &Here::Bot(Box::new(b[0].clone())), &b), says("No Bot is on Ada's list."));
        assert_eq!(run_on("allow", &Here::Bot(Box::new(b[0].clone())), &b), says("Allow whom? /messages allow <Bot>"));
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
