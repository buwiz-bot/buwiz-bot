//! The Agent Hub: how the broker's model sees mini app screens.
//!
//! The Hub lives in the app and reads nsh's tree in the process, so a
//! shipped Bot opens no listener. The broker reaches it over the connection
//! the Bot makes itself: its requests come down `GET /v1/host/ui/stream`,
//! the answers go up `POST /v1/host/ui/reply`, both with the host token.
//! `/v1/host/events` isn't used: it is a broadcast and drops on lag.
//!
//! Each message is a `buwiz-agent` [`Envelope`] or [`Response`]. The one
//! request this slice answers is `snapshot` to the target `bot`, the Hub
//! itself; its answer's `result` is `{"apps": [UiTree, …]}`, a tree for
//! each app with something open, each screen a `page` node. Anything else
//! is refused.
//!
//! Nothing reads the Bot's own windows: only opens that [`opened`] named,
//! and [`filter`] runs on each here, before anything leaves the app. The
//! model's tool, and the per-Bot switch that leaves it off, are the broker's
//! (`hub/tools/ui.rs`).

pub mod filter;

use std::{cell::RefCell, rc::Weak, time::Duration};

use buwiz_agent::{AgentId, Envelope, Op, PlatformKind, Response, Target, UiTree};
use nsh_platform_macos::{AppView, ui_tree::UiNode as Seen};
use serde_json::json;

use crate::mini_v1::{
    host_api::{Broker, HttpBroker},
    native::on_main,
    to_store,
};
use filter::Reading;

const STREAM: &str = "/v1/host/ui/stream";
const REPLY: &str = "/v1/host/ui/reply";

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
    view: Weak<AppView>,
}

thread_local! {
    static OPENS: RefCell<Vec<Open>> = const { RefCell::new(Vec::new()) };
}

/// An open started, on the main thread. `short` is its plugin's.
pub fn opened(n: u64, key: &str, short: &str, plugin_title: &str, app: &str, title: &str, view: Weak<AppView>) {
    let name = if title.is_empty() || title == plugin_title { plugin_title.to_owned() } else { format!("{plugin_title}: {title}") };
    OPENS.with_borrow_mut(|o| o.push(Open { n, key: key.to_owned(), target: short.to_owned(), name, app: app.to_owned(), view }));
}

/// Open `n` ended, on the main thread.
pub fn closed(n: u64) {
    OPENS.with_borrow_mut(|o| o.retain(|open| open.n != n));
}

/// One open as it is now.
struct Surface {
    key: String,
    target: String,
    name: String,
    app: String,
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
                    key: o.key.clone(),
                    target: o.target.clone(),
                    name: o.name.clone(),
                    app: o.app.clone(),
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
    // the stack has one entry for a key: the newest open of it is the one showing
    let top = stack.last().and_then(|key| surfaces.iter().rposition(|s| s.key == *key));
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

/// A screen's id: its app's, numbered from the second on.
fn screen_id(app: &str, before: usize) -> Option<AgentId> {
    let app: String = app.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let id = if before == 0 { format!("screen.{app}") } else { format!("screen.{app}-{}", before + 1) };
    AgentId::parse(&id).ok()
}

fn reply(r: Response) {
    let Ok(body) = serde_json::to_value(&r) else { return };
    std::thread::spawn(move || {
        if let Err(e) = HttpBroker::configured().request("POST", REPLY, Some(body), Duration::from_secs(5)) {
            log::warn!("agent hub: the reply didn't reach the broker: {}", e.code);
        }
    });
}

/// Answers `request`. A look goes through the store, for the stack, then
/// the main thread, for the views.
fn answer(request: Envelope) {
    let Envelope { id, target, op, .. } = request;
    if target.as_str() != "bot" || !matches!(op, Op::Snapshot { .. }) {
        return reply(Response::err(id, target, "the Hub only answers a look at its own target"));
    }
    to_store(move |st| {
        let stack: Vec<String> = st.mini_full.with_untracked(|v| v.iter().map(|o| o.key.clone()).collect());
        on_main(move || {
            let mut r = Response::ok(id, target);
            r.result = Some(json!({"apps": trees(&surfaces(), &stack, Reading::configured())}));
            reply(r);
        });
    });
}

/// Follows the broker's requests for as long as the app runs. This is the
/// Bot's own connection out: nothing here listens.
pub fn start() {
    let run = || {
        let broker = HttpBroker::configured();
        loop {
            let _ = broker.stream(STREAM, |name, data| {
                if name == "ui" {
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
    use super::filter::tests::{NAME, TIN, button, profile};
    use super::*;
    use buwiz_agent::UiNode;

    fn screen_of<'a>(trees: &'a [UiTree], id: &str) -> &'a UiNode {
        trees.iter().flat_map(|t| &t.nodes).find(|n| n.id.as_str() == id).expect("the screen")
    }

    fn surface(key: &str, target: &str, app: &str, name: &str, nodes: Vec<Seen>) -> Surface {
        Surface { key: key.into(), target: target.into(), name: name.into(), app: app.into(), nodes }
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
