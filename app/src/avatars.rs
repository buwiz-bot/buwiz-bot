//! `/avatars`: the packs at a link, and adding avatars from one to "Your designs". The broker
//! reads and checks the link (`POST /avatar/packs`) and never writes the list: that stays here,
//! so it has one writer. The default link is the broker's.

use nsh_core::prelude::*;
use serde_json::{Value, json};

use crate::client;
use crate::live::store;
use crate::store::DesignsAdded;

const HELP: &str = "Use /avatars list, or /avatars install followed by all, a pack or an avatar's name, and a link if it's not the usual one.";

/// What `/avatars` was asked.
#[derive(Debug, PartialEq)]
enum Ask {
    List { link: String },
    Install { pick: String, link: String },
}

/// What follows `/avatars`: a link anywhere in it, and the rest as words. A pack's name can run
/// to several. Everything takes saying `all`: a bare `install` is a command not finished.
fn parse(rest: &str) -> Option<Ask> {
    let rest = rest.trim().trim_start_matches(':');
    let mut words: Vec<&str> = rest.split_whitespace().collect();
    let link = words.iter().position(|w| w.contains("://")).map(|i| words.remove(i)).unwrap_or_default().to_owned();
    match words.split_first() {
        None => Some(Ask::List { link }),
        Some((&"list", [])) => Some(Ask::List { link }),
        Some((&"install", [])) => None,
        Some((&"install", pick)) => Some(Ask::Install { pick: pick.join(" "), link }),
        _ => None,
    }
}

/// The `/avatars` command: `rest` is what followed its name.
pub(crate) fn run(rest: &str) {
    let Some(ask) = parse(rest) else {
        return crate::toast::show(HELP.to_owned());
    };
    Owner::detached().run(|| {
        spawn(async move {
            crate::toast::show(answer(ask).await);
        });
    });
}

async fn answer(ask: Ask) -> String {
    match ask {
        Ask::List { link } => match packs(json!({"link": link, "list": true})).await {
            Ok(v) => listed(&v),
            Err(e) => e,
        },
        Ask::Install { pick, link } => {
            // what's already there can't be told until the list has been read
            if !store().design_loaded.get_untracked() {
                return "Your avatars are still loading. Try again in a moment.".to_owned();
            }
            match packs(json!({"link": link, "pick": pick})).await {
                Ok(v) => {
                    let count = |k: &str| v[k].as_array().map_or(0, Vec::len);
                    let designs = v["designs"].as_array().map_or(&[][..], Vec::as_slice);
                    outcome(store().designs_add(designs), count("refused"), count("failed"))
                }
                Err(e) => e,
            }
        }
    }
}

/// The broker's answer, or what to tell the person.
async fn packs(body: Value) -> Result<Value, String> {
    let Some(v) = client::post_any("/avatar/packs", body).await else {
        return Err("Couldn't read that link.".to_owned());
    };
    if v["ok"] == json!(true) {
        return Ok(v);
    }
    Err(v["error"].as_str().map_or_else(|| "Couldn't read that link.".to_owned(), capitalised))
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| format!("{}{}.", f.to_uppercase(), c.as_str().trim_end_matches('.')))
}

/// `3 avatars`, or `1 avatar`.
fn avatars(n: usize) -> String {
    format!("{n} avatar{}", if n == 1 { "" } else { "s" })
}

fn listed(v: &Value) -> String {
    let titles: Vec<&str> = v["packs"].as_array().map(|p| p.iter().filter_map(|x| x["title"].as_str()).collect()).unwrap_or_default();
    if titles.is_empty() {
        return "No packs at that link.".to_owned();
    }
    format!("Packs: {}. Use /avatars install and a name to add one.", titles.join(", "))
}

/// What came of an install, in a line: `refused` avatars and `failed` whole packs.
fn outcome(got: DesignsAdded, refused: usize, failed: usize) -> String {
    let skipped = got.unusable + refused;
    let mut parts = Vec::new();
    if got.added > 0 {
        parts.push(format!("Added {} to Your designs", avatars(got.added)));
    }
    if got.there > 0 {
        parts.push(format!("{} already there", avatars(got.there)));
    }
    if got.repeated > 0 {
        parts.push(format!("{} listed twice", avatars(got.repeated)));
    }
    if skipped > 0 {
        parts.push(format!("{} couldn't be used", avatars(skipped)));
    }
    if failed > 0 {
        parts.push(format!("{failed} pack{} couldn't be read", if failed == 1 { "" } else { "s" }));
    }
    if parts.is_empty() {
        return "Nothing to add.".to_owned();
    }
    format!("{}.", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(pick: &str, link: &str) -> Option<Ask> {
        Some(Ask::Install { pick: pick.to_owned(), link: link.to_owned() })
    }

    #[test]
    fn what_it_was_asked() {
        let link = "https://github.com/me/mine";
        assert_eq!(parse(""), Some(Ask::List { link: String::new() }));
        assert_eq!(parse(" list"), Some(Ask::List { link: String::new() }));
        assert_eq!(parse(&format!(" list {link}")), Some(Ask::List { link: link.to_owned() }));
        assert_eq!(parse(&format!(" {link}")), Some(Ask::List { link: link.to_owned() }));
        assert_eq!(parse(" install"), None);
        assert_eq!(parse(&format!(" install {link}")), None);
        assert_eq!(parse(" install all"), install("all", ""));
        assert_eq!(parse(" install Silver Harbor"), install("Silver Harbor", ""));
        assert_eq!(parse(&format!(" install {link} fox")), install("fox", link));
        assert_eq!(parse(&format!(":install fox {link}")), install("fox", link));
        assert_eq!(parse(" remove all"), None);
        assert_eq!(parse(" list fox"), None);
    }

    #[test]
    fn what_it_says_after_an_install() {
        let got = |added, there, unusable| DesignsAdded { added, there, unusable, ..Default::default() };
        assert_eq!(outcome(got(3, 0, 0), 0, 0), "Added 3 avatars to Your designs.");
        assert_eq!(outcome(got(1, 2, 0), 0, 0), "Added 1 avatar to Your designs, 2 avatars already there.");
        assert_eq!(outcome(got(0, 1, 0), 0, 0), "1 avatar already there.");
        assert_eq!(outcome(got(1, 0, 1), 2, 0), "Added 1 avatar to Your designs, 3 avatars couldn't be used.");
        assert_eq!(outcome(got(0, 0, 0), 0, 0), "Nothing to add.");
        assert_eq!(outcome(got(0, 0, 0), 0, 1), "1 pack couldn't be read.");
        assert_eq!(outcome(got(2, 0, 0), 0, 2), "Added 2 avatars to Your designs, 2 packs couldn't be read.");
        let twice = DesignsAdded { added: 1, repeated: 1, ..Default::default() };
        assert_eq!(outcome(twice, 0, 0), "Added 1 avatar to Your designs, 1 avatar listed twice.");
    }

    #[test]
    fn what_it_says_of_a_list() {
        assert_eq!(listed(&json!({"packs": []})), "No packs at that link.");
        let v = json!({"packs": [{"title": "Animals"}, {"title": "Pets"}]});
        assert_eq!(listed(&v), "Packs: Animals, Pets. Use /avatars install and a name to add one.");
    }

    #[test]
    fn a_broker_error_reads_as_a_sentence() {
        assert_eq!(capitalised("no packs at that link"), "No packs at that link.");
    }
}
