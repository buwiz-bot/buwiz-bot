//! `/vault`: the saved logins, from the composer. A person's command only:
//! `slash::dispatch` runs it on their own Enter, and a chip's or a card's
//! words that read so go to the AI as plain text (`slash::reads_as_command`).
//! Nothing here reaches the model, and no secret is ever taken from the line.
//!
//! - `/vault`: Settings › Logins, which lists sites and usernames, never a secret.
//! - `/vault add <site>`: the Add form for that site. The password is typed
//!   into the form's masked field; a line with more after the site is refused.
//! - `/vault share <site> with <Bot>`, `/vault unshare <site> with <Bot>`:
//!   whether that Bot is offered the site's login. Two logins for one site
//!   ask which (`… as <username> with <Bot>`), never a guess.
//! - `/vault share all`, `/vault share all off`: the global switch.

use super::{SHARE_ALL_MEANS, SiteLogin, SiteMatch, site_match, site_origin, vault};
use crate::live::store;
use crate::slash::{Pick, PopRow};
use crate::store::RoxBot;

const USAGE: &str = "/vault, /vault add <site>, /vault share <site> with <Bot>, /vault unshare <site> with <Bot>, /vault share all, /vault share all off";
const NO_SECRET_HERE: &str = "/vault add takes a site only. Type the password in the form it opens, never on the command line.";

/// What follows `/vault`, read whole.
#[derive(Debug, PartialEq)]
enum Line<'a> {
    Open,
    Add(&'a str),
    ShareAll(bool),
    /// `share` (`on`) or `unshare`, a site, maybe a username, a Bot.
    Share { on: bool, site: &'a str, user: Option<&'a str>, bot: &'a str },
    Say(&'static str),
}

/// `text` split at the last ` <word> ` (any case): what's before and after.
fn split_word<'a>(text: &'a str, word: &str) -> Option<(&'a str, &'a str)> {
    let lower = text.to_ascii_lowercase();
    let at = lower.rfind(&format!(" {word} "))?;
    Some((text[..at].trim(), text[at + word.len() + 2..].trim()))
}

fn parse(rest: &str) -> Line<'_> {
    let line = rest.trim();
    let (verb, tail) = line.split_once(char::is_whitespace).map(|(v, t)| (v, t.trim())).unwrap_or((line, ""));
    let is = |w: &str, word: &str| w.eq_ignore_ascii_case(word);
    match verb {
        "" => Line::Open,
        v if is(v, "list") && tail.is_empty() => Line::Open,
        v if is(v, "add") => match tail.split_whitespace().count() {
            0 => Line::Add(""),
            1 => Line::Add(tail),
            _ => Line::Say(NO_SECRET_HERE),
        },
        v if is(v, "share") && (is(tail, "all") || is(tail, "all on")) => Line::ShareAll(true),
        v if is(v, "share") && is(tail, "all off") => Line::ShareAll(false),
        v if is(v, "unshare") && is(tail, "all") => Line::Say("To stop offering every login to every Bot: /vault share all off"),
        v if is(v, "share") || is(v, "unshare") => {
            let on = is(v, "share");
            let Some((what, bot)) = split_word(tail, "with").filter(|(w, b)| !w.is_empty() && !b.is_empty()) else {
                return Line::Say(if on { "Share with whom? /vault share <site> with <Bot>" } else { "Stop sharing with whom? /vault unshare <site> with <Bot>" });
            };
            let (site, user) = match what.to_ascii_lowercase().find(" as ") {
                Some(at) => (what[..at].trim(), Some(what[at + 4..].trim()).filter(|u| !u.is_empty())),
                None => (what, None),
            };
            if site.split_whitespace().count() != 1 {
                return Line::Say("Name one site: /vault share <site> with <Bot>");
            }
            Line::Share { on, site, user, bot }
        }
        _ => Line::Say(USAGE),
    }
}

/// What a line comes to.
enum Outcome {
    Say(String),
    Open,
    Add(String),
    ShareAll(bool),
    Share { id: String, bot: String, on: bool, said: String },
    /// More than one login fits: these rows ask which, with this question.
    Ask(Vec<PopRow>, String),
}

/// The one Bot called `who` (its name, any case, or its id).
fn named<'a>(who: &str, bots: &'a [RoxBot]) -> Result<&'a RoxBot, String> {
    if let Some(b) = bots.iter().find(|b| b.id == who) {
        return Ok(b);
    }
    let who_l = who.trim().to_lowercase();
    let mut found = bots.iter().filter(|b| b.name.trim().to_lowercase() == who_l);
    match (found.next(), found.next()) {
        (Some(b), None) => Ok(b),
        (Some(_), Some(_)) => Err(format!("More than one Bot is called {who}: rename one first.")),
        _ => Err(format!("No Bot is called {who}.")),
    }
}

fn row(fill: String, label: String, detail: String) -> PopRow {
    PopRow { pick: Pick::Fill(fill), label, hint: String::new(), detail, bot: String::new() }
}

fn outcome(line: Line, rows: &[SiteLogin], bots: &[RoxBot], share_all: bool) -> Outcome {
    match line {
        Line::Say(s) => Outcome::Say(s.to_owned()),
        Line::Open => Outcome::Open,
        Line::Add("") => Outcome::Add(String::new()),
        Line::Add(site) => match site_origin(site) {
            Some(s) => Outcome::Add(s),
            None => Outcome::Say(format!("{site} isn't a site.")),
        },
        Line::ShareAll(on) => Outcome::ShareAll(on),
        Line::Share { on, site, user, bot } => {
            let b = match named(bot, bots) {
                Ok(b) => b,
                Err(why) => return Outcome::Say(why),
            };
            let Some(site) = site_origin(site) else { return Outcome::Say(format!("{site} isn't a site.")) };
            let found: Vec<&SiteLogin> = rows
                .iter()
                .filter(|r| site_match(&r.origin, true, &site) == SiteMatch::Exact)
                .filter(|r| user.is_none_or(|u| r.username.eq_ignore_ascii_case(u)))
                .collect();
            let verb = if on { "share" } else { "unshare" };
            let r = match found.as_slice() {
                [] => {
                    let who = user.map(|u| format!(" as {u}")).unwrap_or_default();
                    return Outcome::Say(format!("No login is saved for {site}{who}. /vault add {site} adds one."));
                }
                [r] => *r,
                many => {
                    let asks = many
                        .iter()
                        .map(|r| row(format!("/vault {verb} {site} as {} with {}", r.username, b.name), r.username.clone(), format!("Saved for {}", r.origin)))
                        .collect();
                    return Outcome::Ask(asks, format!("{} logins are saved for {site}: which one?", many.len()));
                }
            };
            let who = format!("{} on {}", r.username, r.origin);
            let said = match (on, r.shared_with.contains(&b.id)) {
                (true, true) => format!("{who} is shared with {} already.", b.name),
                (true, false) => format!("Shared {who} with {}. It's offered on {}'s own computer only, and Touch ID is asked every time.", b.name, b.name),
                (false, false) if share_all => format!("{who} isn't shared with {} by name, but Share all is on, so every Bot is offered it. /vault share all off stops that.", b.name),
                (false, false) => format!("{who} isn't shared with {}.", b.name),
                (false, true) if share_all => format!("{} isn't offered {who} by name any more, but Share all is on, so every Bot still is. /vault share all off stops that.", b.name),
                (false, true) => format!("{} isn't offered {who} any more.", b.name),
            };
            Outcome::Share { id: r.id.clone(), bot: b.id.clone(), on, said }
        }
    }
}

/// [`crate::slash`]'s `run`: `rest` is what follows `/vault`.
pub(crate) fn run(rest: &str) {
    let v = vault();
    let bots = store().bots.get_untracked();
    let say = |s: String| crate::toast::show(s);
    match outcome(parse(rest), &v.list(), &bots, v.share_all()) {
        Outcome::Say(s) => say(s),
        Outcome::Open => super::open_page(),
        Outcome::Add(site) => super::open_add(&site),
        Outcome::ShareAll(on) => match v.set_share_all(on).inspect(|()| super::changed()) {
            Ok(()) => say(if on { format!("Share all is on. {SHARE_ALL_MEANS}") } else { "Share all is off: a login is offered only to the Bots it's shared with.".to_owned() }),
            Err(e) => say(format!("Couldn't change Share all: {e}")),
        },
        Outcome::Share { id, bot, on, said } => match v.share(&id, &bot, on).inspect(|_| super::changed()) {
            Ok(_) => say(said),
            Err(e) => say(format!("Couldn't change who it's shared with: {e}")),
        },
        Outcome::Ask(rows, head) => {
            say(head.clone());
            crate::slash::ask(rows, head);
        }
    }
}

/// [`crate::slash`]'s `menu`: the list past `/vault `. Only sites and names, never a secret.
pub(crate) fn menu(rest: &str) -> (Vec<PopRow>, String) {
    let line = rest.trim_start();
    let starts = |word: &str, q: &str| word.to_lowercase().starts_with(&q.to_lowercase());
    let (verb, tail) = match line.split_once(char::is_whitespace) {
        Some((v, t)) => (v, Some(t.trim_start())),
        None => (line, None),
    };
    let Some(tail) = tail else {
        let subs = [
            ("add ", "add", "Add a login for a site: the form opens"),
            ("share ", "share", "Offer a site's login to a Bot"),
            ("unshare ", "unshare", "Stop offering a site's login to a Bot"),
            ("share all", "share all", "Offer every login to every Bot"),
            ("share all off", "share all off", "Offer a login only to the Bots it's shared with"),
        ];
        let rows = subs.iter().filter(|(_, w, _)| starts(w, verb)).map(|(f, w, d)| row(format!("/vault {f}"), (*w).to_owned(), (*d).to_owned())).collect();
        return (rows, String::new());
    };
    if !(verb.eq_ignore_ascii_case("share") || verb.eq_ignore_ascii_case("unshare")) {
        return (Vec::new(), String::new());
    }
    let logins = vault().list();
    match split_word(&format!("{tail} "), "with") {
        // past `with`: the Bots
        Some((what, q)) => {
            let rows = store()
                .bots
                .get_untracked()
                .iter()
                .filter(|b| starts(&b.name, q))
                .map(|b| PopRow { pick: Pick::Fill(format!("/vault {verb} {what} with {}", b.name)), label: b.name.clone(), hint: String::new(), detail: format!("{verb} with {}", b.name), bot: b.id.clone() })
                .collect();
            (rows, String::new())
        }
        // the sites saved, each once
        None => {
            let mut sites: Vec<&str> = logins.iter().map(|r| r.origin.as_str()).filter(|s| starts(s, tail)).collect();
            sites.dedup();
            (sites.into_iter().map(|s| row(format!("/vault {verb} {s} with "), s.to_owned(), "A saved site".to_owned())).collect(), String::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn bot(id: &str, name: &str) -> RoxBot {
        RoxBot { id: id.into(), name: name.into(), ..Default::default() }
    }

    fn login(id: &str, site: &str, user: &str, shared: &[&str]) -> SiteLogin {
        SiteLogin { id: id.into(), origin: site.into(), username: user.into(), exact: true, shared_with: shared.iter().map(|s| (*s).to_owned()).collect::<BTreeSet<_>>(), ..Default::default() }
    }

    #[test]
    fn the_lines_read_as_they_say() {
        assert_eq!(parse(""), Line::Open);
        assert_eq!(parse("  list "), Line::Open);
        assert_eq!(parse(" add shop.example"), Line::Add("shop.example"));
        assert_eq!(parse(" add"), Line::Add(""));
        assert_eq!(parse(" share all"), Line::ShareAll(true));
        assert_eq!(parse(" SHARE ALL OFF"), Line::ShareAll(false));
        assert_eq!(parse(" share shop.example with Nova"), Line::Share { on: true, site: "shop.example", user: None, bot: "Nova" });
        assert_eq!(parse(" unshare shop.example as kid@example.com with Big Nova"), Line::Share { on: false, site: "shop.example", user: Some("kid@example.com"), bot: "Big Nova" });
        assert!(matches!(parse(" share shop.example"), Line::Say(_)));
        assert!(matches!(parse(" unshare all"), Line::Say(_)));
        assert!(matches!(parse(" frobnicate"), Line::Say(_)));
    }

    #[test]
    fn add_never_takes_a_password_from_the_line() {
        assert_eq!(parse(" add shop.example fake-secret-marker"), Line::Say(NO_SECRET_HERE));
        assert!(!NO_SECRET_HERE.contains("fake-secret-marker"));
        assert!(matches!(outcome(parse(" add https://Shop.Example/login"), &[], &[], false), Outcome::Add(ref s) if s == "shop.example"));
    }

    #[test]
    fn share_finds_one_login_and_one_bot_and_asks_when_two_fit() {
        let bots = [bot("nova", "Nova"), bot("orbit", "Orbit")];
        let rows = [login("sl_1", "shop.example", "kid", &[]), login("sl_2", "bank.example", "ada", &["nova"]), login("sl_3", "bank.example", "bob", &[])];
        match outcome(parse(" share shop.example with nova"), &rows, &bots, false) {
            Outcome::Share { id, bot, on, said } => {
                assert_eq!((id.as_str(), bot.as_str(), on), ("sl_1", "nova", true));
                assert!(said.starts_with("Shared kid on shop.example with Nova."), "{said}");
            }
            _ => panic!("not the outcome this line names"),
        }
        // two logins for one site: asked, never guessed
        match outcome(parse(" share bank.example with Orbit"), &rows, &bots, false) {
            Outcome::Ask(asks, head) => {
                assert_eq!(head, "2 logins are saved for bank.example: which one?");
                let fills: Vec<_> = asks.iter().map(|r| match &r.pick { Pick::Fill(t) => t.clone(), _ => String::new() }).collect();
                assert_eq!(fills, ["/vault share bank.example as ada with Orbit", "/vault share bank.example as bob with Orbit"]);
            }
            _ => panic!("not the outcome this line names"),
        }
        assert!(matches!(outcome(parse(" share bank.example as bob with Orbit"), &rows, &bots, false), Outcome::Share { ref id, .. } if id == "sl_3"));
        assert!(matches!(outcome(parse(" unshare bank.example as ada with Nova"), &rows, &bots, false), Outcome::Share { ref id, on: false, .. } if id == "sl_2"));
        assert!(matches!(outcome(parse(" share nowhere.example with Nova"), &rows, &bots, false), Outcome::Say(ref s) if s.starts_with("No login is saved for nowhere.example")));
        assert!(matches!(outcome(parse(" share shop.example with Nobody"), &rows, &bots, false), Outcome::Say(ref s) if s == "No Bot is called Nobody."));
        let twins = [bot("a", "Nova"), bot("b", "nova")];
        assert!(matches!(outcome(parse(" share shop.example with Nova"), &rows, &twins, false), Outcome::Say(ref s) if s.starts_with("More than one Bot")));
        // unsharing under share-all says the Bot is still offered it
        assert!(matches!(outcome(parse(" unshare bank.example as ada with Nova"), &rows, &bots, true), Outcome::Share { ref said, .. } if said.contains("Share all is on")));
    }
}
