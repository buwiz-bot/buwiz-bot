//! `/network` and the Computer tab's Network section: the browser's network of the open Bot's
//! computer. A person's command only: `slash::dispatch` runs it on their own Enter, and a chip's or
//! a card's words that read so go to the AI as plain text (`slash::reads_as_command`).
//!
//! The settings are the **computer's**, not the Bot's: a computer has one proxy and one tunnel,
//! and every Bot on it gets the same rules. Every reply and the section say so, and say what the
//! rules don't cover (commands the agent runs). `--all` sends a change to every computer.
//!
//! - `/network`: where the open Bot's computer stands.
//! - `/network tunnel on|off`: browse out through this Mac, or from the computer itself.
//! - `/network allow|unallow|block|unblock <site>`: the lists. A site covers every name under it.
//! - `/network mode all|listed`: every site but the blocked ones, or only the allowed ones.
//! - `/network rule ask|allow|never`: not built yet; the broker says so and changes nothing.
//!
//! The broker (`/v1/host/computer/bot/{bot}/network`) checks every site again and is the one rule;
//! a word that doesn't look like a site is refused here in a fixed sentence, never shown back.

use nsh_base::input;
use nsh_core::prelude::*;
use serde_json::{json, Value};

use crate::client;
use crate::homes;
use crate::live::store;
use crate::slash::{Pick, PopRow};
use crate::store::spawn_detached;
use crate::theme;

const USAGE: &str = "/network, /network tunnel on|off, /network allow|unallow|block|unblock <site>, /network mode all|listed, /network rule ask|allow|never; add --all for every computer";
/// For a word that isn't a site: fixed, so whatever was typed is never shown back.
const NOT_A_SITE: &str = "That doesn't look like a site (like example.com), so nothing was changed. A site covers every name under it, so no * is needed; an international name goes in its xn-- form.";
const NO_BOT: &str = "Open a Bot's chat first: the network settings are its computer's.";
const NO_BROKER: &str = "Buwiz Bot's broker isn't answering, so nothing was changed.";

/// What follows `/network`, read whole.
#[derive(Debug, PartialEq)]
enum Line {
    Show,
    Change { op: &'static str, value: String, all: bool },
    Say(&'static str),
}

fn parse(rest: &str) -> Line {
    let words: Vec<&str> = rest.split_whitespace().collect();
    let all = words.iter().any(|w| w.eq_ignore_ascii_case("--all"));
    let words: Vec<&str> = words.into_iter().filter(|w| !w.eq_ignore_ascii_case("--all")).collect();
    let is = |w: &str, word: &str| w.eq_ignore_ascii_case(word);
    let change = |op: &'static str, value: String| Line::Change { op, value, all };
    match words.as_slice() {
        [] if !all => Line::Show,
        [] => Line::Say("--all goes with a change: /network block <site> --all"),
        [v] if is(v, "status") && !all => Line::Show,
        [v, on] if is(v, "tunnel") && (is(on, "on") || is(on, "off")) => change("tunnel", on.to_ascii_lowercase()),
        [v, ..] if is(v, "tunnel") => Line::Say("/network tunnel on, or /network tunnel off"),
        [v, m] if is(v, "mode") && (is(m, "all") || is(m, "listed")) => change("mode", m.to_ascii_lowercase()),
        [v, ..] if is(v, "mode") => Line::Say("/network mode all (every site but the blocked ones), or /network mode listed (only the allowed ones)"),
        [v, r] if is(v, "rule") && ["ask", "allow", "never"].iter().any(|w| is(r, w)) => change("rule", r.to_ascii_lowercase()),
        [v, ..] if is(v, "rule") => Line::Say("/network rule ask, allow or never"),
        [v, site] if ["allow", "unallow", "block", "unblock"].iter().any(|w| is(v, w)) => {
            let op = ["allow", "unallow", "block", "unblock"].into_iter().find(|w| is(v, w)).unwrap_or("allow");
            match crate::vault::plausible_site(site) {
                Some(site) => change(op, site),
                None => Line::Say(NOT_A_SITE),
            }
        }
        [v, ..] if ["allow", "unallow", "block", "unblock"].iter().any(|w| is(v, w)) => Line::Say("Name one site: /network block example.com"),
        _ => Line::Say(USAGE),
    }
}

/// What the broker said of a computer's network, for a reply and the section.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Net {
    /// the broker's own sentence when it couldn't say (no computer, unreadable)
    pub message: String,
    pub has_computer: bool,
    pub supported: bool,
    pub why: String,
    pub tunnel_made: bool,
    pub tunnel: bool,
    pub listed: bool,
    pub allow: Vec<String>,
    pub block: Vec<String>,
    /// the client on this Mac: off, attaching, attached, retrying, refused, stopped
    pub client: String,
    pub client_error: String,
    pub awake: bool,
    pub in_force: Option<bool>,
    pub can_browse: Option<bool>,
    /// what the computer's proxy did, as counts: (loaded, blocked)
    pub counts: Option<(u64, u64)>,
    pub notes: Vec<String>,
}

impl Net {
    pub(crate) fn parse(v: &Value) -> Self {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        let b = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        let list = |k: &str| v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
        let n = |k: &str| v.pointer(&format!("/counts/{k}")).and_then(Value::as_u64).unwrap_or(0);
        Self {
            message: s("message"),
            has_computer: v.get("state").and_then(Value::as_str) == Some("computer"),
            supported: b("supported"),
            why: s("why"),
            tunnel_made: b("tunnelMade"),
            tunnel: b("tunnel"),
            listed: s("mode") == "listed",
            allow: list("allow"),
            block: list("block"),
            client: v.pointer("/client/state").and_then(Value::as_str).unwrap_or("off").to_owned(),
            client_error: v.pointer("/client/lastError").and_then(Value::as_str).unwrap_or_default().to_owned(),
            awake: b("awake"),
            in_force: v.get("inForce").and_then(Value::as_bool),
            can_browse: v.get("canBrowse").and_then(Value::as_bool),
            counts: v.get("counts").filter(|c| c.is_object()).map(|_| (n("direct") + n("tunneled"), n("blocked") + n("not_listed"))),
            notes: list("notes"),
        }
    }

    /// The tunnel's state in words a person can act on.
    pub(crate) fn tunnel_words(&self) -> String {
        if !self.tunnel {
            return "Tunnel off: the computer dials sites itself.".to_owned();
        }
        match self.client.as_str() {
            "attached" => "Tunnel on: the browser goes out through this Mac.".to_owned(),
            "refused" => "Tunnel on, and the computer refused this Mac's side, so its browser reaches no site. Turn the tunnel off and on again; if it stays refused, make the computer again.".to_owned(),
            _ if !self.awake => "Tunnel on: it attaches when the computer wakes.".to_owned(),
            _ => "Tunnel on, and this Mac's side isn't attached yet, so the browser on this computer can't reach any site until it is.".to_owned(),
        }
    }

    /// The whole state, as `/network` says it.
    pub(crate) fn summary(&self) -> String {
        if !self.has_computer {
            return if self.message.is_empty() { "This Bot has no computer yet.".to_owned() } else { self.message.clone() };
        }
        if !self.supported {
            return self.why.clone();
        }
        let mode = if self.listed { "Only allowed sites load" } else { "Every site loads but the blocked ones" };
        let list = |l: &[String]| if l.is_empty() { "none".to_owned() } else { l.join(", ") };
        let mut out = format!("{} {mode}. Allowed: {}. Blocked: {}.", self.tunnel_words(), list(&self.allow), list(&self.block));
        for n in &self.notes {
            out.push(' ');
            out.push_str(n);
        }
        out
    }
}

fn path(bot: &str) -> String {
    format!("/v1/host/computer/bot/{}/network", client::urlenc(bot))
}

/// What one change came to, in a sentence: the computer's, or each computer's with `--all`.
fn reply_words(v: &Value) -> String {
    if let Some(results) = v.get("results").and_then(Value::as_array) {
        if results.is_empty() {
            return "There are no computers to change.".to_owned();
        }
        let lines: Vec<String> = results
            .iter()
            .map(|r| {
                let bots = r.get("bots").and_then(Value::as_array).map(|b| b.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                let who = if bots.is_empty() { r["computer"].as_str().unwrap_or_default().to_owned() } else { format!("{bots}'s computer") };
                format!("{who}: {}", r["message"].as_str().unwrap_or_default())
            })
            .collect();
        return format!("{} {LIMIT}", lines.join(" "));
    }
    let said = v.get("message").and_then(Value::as_str).unwrap_or_default();
    let shared = v.pointer("/network/bots").and_then(Value::as_array).map(|b| b.iter().filter_map(|b| b.get("name").and_then(Value::as_str)).collect::<Vec<_>>()).unwrap_or_default();
    if v.get("ok") == Some(&json!(true)) && shared.len() > 1 {
        format!("{said} This is the computer's setting: every Bot on it ({}) gets it. {LIMIT}", shared.join(", "))
    } else if v.get("ok") == Some(&json!(true)) {
        format!("{said} {LIMIT}")
    } else {
        said.to_owned()
    }
}

/// What every reply that changes something ends with.
pub(crate) const LIMIT: &str = "The rules cover the browser only: commands the agent runs aren't covered.";

/// [`crate::slash`]'s `run`: `rest` is what follows `/network`. Nothing reaches the model.
pub(crate) fn run(rest: &str) {
    let line = parse(rest);
    let say = |s: String| crate::toast::show(s);
    let Some(bot) = store().active_bot_untracked() else { return say(NO_BOT.to_owned()) };
    match line {
        Line::Say(s) => say(s.to_owned()),
        Line::Show => spawn_detached(async move {
            match client::get_any(&path(&bot.id)).await {
                Some(v) => {
                    let net = Net::parse(&v);
                    show(&bot.id, net.clone());
                    say(format!("{}: {}", bot.name, net.summary()));
                }
                None => say("Buwiz Bot's broker isn't answering.".to_owned()),
            }
        }),
        Line::Change { op, value, all } => spawn_detached(async move {
            let said = match client::post_status(&path(&bot.id), json!({"op": op, "value": value, "all": all})).await {
                Some((_, v)) => {
                    if let Some(n) = v.get("network") {
                        show(&bot.id, Net::parse(n));
                    }
                    reply_words(&v)
                }
                None => NO_BROKER.to_owned(),
            };
            say(said);
        }),
    }
}

/// [`crate::slash`]'s `menu`: the list past `/network `.
pub(crate) fn menu(rest: &str) -> (Vec<PopRow>, String) {
    let q = rest.trim_start().to_lowercase();
    let subs = [
        ("tunnel on", "Browse out through this Mac"),
        ("tunnel off", "The computer dials sites itself"),
        ("allow ", "Allow a site and every name under it"),
        ("unallow ", "Take a site off the allow list"),
        ("block ", "Block a site and every name under it"),
        ("unblock ", "Take a site off the block list"),
        ("mode all", "Every site but the blocked ones"),
        ("mode listed", "Only the allowed sites"),
    ];
    let rows = subs
        .iter()
        .filter(|(w, _)| w.starts_with(&q) || q.is_empty())
        .map(|(w, d)| PopRow { pick: Pick::Fill(format!("/network {w}")), label: w.trim().to_owned(), hint: String::new(), detail: (*d).to_owned(), bot: String::new() })
        .collect();
    (rows, String::new())
}

thread_local! {
    /// The section's state: whose, what the broker last said, and the site being typed.
    static SECTION: (Signal<String>, Signal<Net>, Signal<String>) = Owner::detached().run(|| (signal(String::new()), signal(Net::default()), signal(String::new())));
}

/// What the section shows for `bot`.
pub(crate) fn show(bot: &str, net: Net) {
    SECTION.with(|(who, n, _)| {
        who.set(bot.to_owned());
        n.set(net);
    });
}

/// Asks the broker again for `bot`'s computer, for the section.
pub(crate) fn refresh(bot: String) {
    spawn_detached(async move {
        if let Some(v) = client::get_any(&path(&bot)).await {
            show(&bot, Net::parse(&v));
        }
    });
}

fn change(op: &'static str, value: String) {
    let Some(bot) = SECTION.with(|(who, _, _)| Some(who.get_untracked()).filter(|b| !b.is_empty())) else { return };
    spawn_detached(async move {
        let said = match client::post_status(&path(&bot), json!({"op": op, "value": value, "all": false})).await {
            Some((_, v)) => {
                if let Some(n) = v.get("network") {
                    show(&bot, Net::parse(n));
                }
                reply_words(&v)
            }
            None => NO_BROKER.to_owned(),
        };
        crate::toast::show(said);
    });
}

fn button(label: String, words: &str, press: impl Fn() + 'static) -> impl IntoElement {
    homes::small_button(label, words.to_owned(), press).role(AccessibilityRole::Button)
}

fn switch(name: &str, on: bool, press: impl Fn() + 'static) -> impl IntoElement {
    let word = if on { "on" } else { "off" };
    view()
        .role(AccessibilityRole::Switch)
        .label(crate::tools::switch_name(name, word))
        .accessibility_state(AccessibilityState { checked: Some(if on { nsh_core::Checked::True } else { nsh_core::Checked::False }), ..Default::default() })
        .px_2()
        .py_1()
        .rounded(px(999.0))
        .bg(if on { theme::ACCENT } else { theme::HOVER })
        .on_press(press)
        .child(text(if on { "On" } else { "Off" }).text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(if on { theme::ON_FILL } else { theme::MUTED }))
}

fn list_rows(name: &'static str, undo: &'static str, sites: &[String]) -> AnyElement {
    let mut out = view().pt(px(6.0)).child(homes::line(format!("{name}:")));
    if sites.is_empty() {
        out = out.child(homes::line("none".to_owned()));
    }
    for site in sites {
        let s = site.clone();
        out = out.child(view().flex_row().items_center().gap_2().pt(px(4.0)).child(text(site.clone()).text_size(px(12.0)).text_color(theme::TEXT)).child(button(format!("Remove {site} from {name}"), "Remove", move || change(undo, s.clone()))));
    }
    out.into_element()
}

/// The section under the Computer tab: the computer's network, as the broker last said it.
pub(crate) fn section() -> impl IntoElement {
    dynamic(move || {
        let (net, typed) = SECTION.with(|(_, n, t)| (n.get(), *t));
        let mut out = view().test_id("network-section").pt(px(16.0)).child(text("Browser network").text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(theme::TEXT));
        if !net.has_computer {
            return out.into_element();
        }
        if !net.supported {
            return out.child(view().test_id("network-why").child(homes::line(net.why.clone()).pt(px(6.0)))).into_element();
        }
        let tunnel_on = net.tunnel;
        let mut tunnel_row = view().flex_row().items_center().gap_2().pt(px(8.0)).child(text("Tunnel through this Mac").text_size(px(12.0)).text_color(theme::TEXT));
        if net.tunnel_made {
            tunnel_row = tunnel_row.child(switch("Tunnel through this Mac", tunnel_on, move || change("tunnel", if tunnel_on { "off" } else { "on" }.to_owned())));
        }
        out = out.child(tunnel_row).child(view().test_id("network-tunnel-words").child(homes::line(net.tunnel_words()).pt(px(4.0))));
        if !net.client_error.is_empty() && net.tunnel && net.client != "attached" {
            out = out.child(homes::line(format!("Last trouble: {}", net.client_error)).pt(px(4.0)));
        }
        let listed = net.listed;
        out = out.child(
            view()
                .flex_row()
                .gap_2()
                .pt(px(8.0))
                .child(switch("Only allowed sites", listed, move || change("mode", if listed { "all" } else { "listed" }.to_owned())))
                .child(text(if listed { "Only allowed sites load, never a blocked one" } else { "Every site loads but the blocked ones" }).text_size(px(12.0)).text_color(theme::MUTED)),
        );
        out = out.child(list_rows("Allowed", "unallow", &net.allow)).child(list_rows("Blocked", "unblock", &net.block));
        let field = view().h(px(32.0)).rounded(px(8.0)).bg(theme::HOVER).px(px(10.0)).justify_center().child(
            input()
                .test_id("network-site")
                .label("A site, like example.com")
                .value(move || typed.get())
                .placeholder("A site, like example.com")
                .placeholder_color(|| color(theme::GHOST))
                .text_size(px(12.0))
                .text_color(theme::TEXT)
                .h(px(32.0))
                .on_change(move |t: &str| typed.set(t.to_owned())),
        );
        let add = move |op: &'static str| {
            move || {
                let raw = typed.get_untracked();
                match crate::vault::plausible_site(&raw) {
                    Some(site) => {
                        typed.set(String::new());
                        change(op, site);
                    }
                    None => crate::toast::show(NOT_A_SITE.to_owned()),
                }
            }
        };
        out = out.child(view().pt(px(8.0)).child(field)).child(view().flex_row().gap_2().pt(px(6.0)).child(button("Allow this site".to_owned(), "Allow", add("allow"))).child(button("Block this site".to_owned(), "Block", add("block"))));
        if let Some((loaded, blocked)) = net.counts {
            out = out.child(homes::line(format!("Since it started: {loaded} connections made, {blocked} refused by these rules.")).pt(px(8.0)));
        }
        for n in &net.notes {
            out = out.child(view().test_id("network-note").child(homes::line(n.clone()).pt(px(6.0))));
        }
        out.into_element()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_read_as_they_say() {
        assert_eq!(parse(""), Line::Show);
        assert_eq!(parse(" status"), Line::Show);
        assert_eq!(parse(" tunnel ON"), Line::Change { op: "tunnel", value: "on".into(), all: false });
        assert_eq!(parse(" block www.Facebook.com --all"), Line::Change { op: "block", value: "www.facebook.com".into(), all: true });
        assert_eq!(parse(" --all unblock shop.example"), Line::Change { op: "unblock", value: "shop.example".into(), all: true });
        assert_eq!(parse(" mode listed"), Line::Change { op: "mode", value: "listed".into(), all: false });
        assert_eq!(parse(" rule never"), Line::Change { op: "rule", value: "never".into(), all: false });
        assert!(matches!(parse(" --all"), Line::Say(_)));
        assert!(matches!(parse(" tunnel maybe"), Line::Say(_)));
        assert!(matches!(parse(" frobnicate"), Line::Say(USAGE)));
        assert!(matches!(parse(" allow a.example b.example"), Line::Say(_)));
    }

    #[test]
    fn a_word_that_isnt_a_site_is_refused_and_never_shown_back() {
        for line in [" allow *", " block hunter2", " unallow p@ss.word", " block *.example.com", " allow ''"] {
            assert_eq!(parse(line), Line::Say(NOT_A_SITE), "{line}");
        }
        assert!(!NOT_A_SITE.contains("hunter2"));
    }

    #[test]
    fn a_status_says_where_it_stands_and_what_it_doesnt_cover() {
        let v = json!({"ok": true, "state": "computer", "supported": true, "tunnelMade": true, "tunnel": true, "mode": "listed", "allow": ["shop.example"], "block": [], "awake": true,
            "client": {"state": "retrying", "lastError": "couldn't attach: connection refused"}, "counts": {"direct": 2, "tunneled": 3, "blocked": 1, "not_listed": 4},
            "notes": ["These are this computer's rules: every Bot on it (Ann, Bea) gets them.", "These rules cover the browser on this computer's screens. Commands the agent runs aren't covered: the network wall still applies to them."]});
        let n = Net::parse(&v);
        assert_eq!(n.counts, Some((5, 5)));
        let said = n.summary();
        assert!(said.contains("can't reach any site until it is"), "{said}");
        assert!(said.contains("Only allowed sites load") && said.contains("Allowed: shop.example. Blocked: none."), "{said}");
        assert!(said.contains("every Bot on it (Ann, Bea)") && said.contains("Commands the agent runs aren't covered"), "{said}");
        let cloud = Net::parse(&json!({"state": "computer", "supported": false, "why": "This computer browses from its provider's network: browsing rules and the tunnel apply to computers on this Mac."}));
        assert!(cloud.summary().starts_with("This computer browses from its provider's network"));
    }

    #[test]
    fn a_reply_names_who_shares_the_setting_and_every_computer_with_all() {
        let one = json!({"ok": true, "message": "facebook.com and every name under it are on the block list. It's in force now.", "network": {"bots": [{"name": "Ann"}, {"name": "Bea"}]}});
        let said = reply_words(&one);
        assert!(said.contains("every Bot on it (Ann, Bea) gets it") && said.ends_with(LIMIT), "{said}");
        let all = json!({"ok": false, "results": [{"computer": "cmp_1", "bots": ["Ann"], "ok": true, "message": "Done."}, {"computer": "cmp_2", "bots": ["Cy"], "ok": false, "message": "This computer browses from its provider's network."}]});
        let said = reply_words(&all);
        assert!(said.starts_with("Ann's computer: Done. Cy's computer: This computer browses"), "{said}");
        let refused = json!({"ok": false, "message": "The network rule (allow, ask or never) isn't built yet, so nothing was changed."});
        assert_eq!(reply_words(&refused), "The network rule (allow, ask or never) isn't built yet, so nothing was changed.");
    }
}
