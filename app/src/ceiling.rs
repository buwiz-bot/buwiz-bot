//! Details › "Tools for this Bot": a bot's tool ceiling, the rows its owner
//! switches on and off. Wire shapes from opengrok-server
//! `crates/opengrok-server/src/agui/ceiling.rs` (`GET`/`PUT
//! /coworkers/{id}/ceiling`; both answer `{"tools":[{name,kind,description,
//! enabled,available?,label?,connector?}],"version"}`, the PUT body is
//! `{"enabled":[name],"version"}` and 409 means the version is stale), with
//! the Buwiz Bot `Bot.id` in the `{coworker_id}` slot. The client shapes and
//! the card's moves follow NativeChat: `opengrok/src/opengrok/client.rs`
//! (`CoworkerCeiling`, `CeilingRow`, `CeilingKind`) and
//! `opengrok/src/state.rs` (`ToolCeiling`, `CeilingCard`,
//! `refresh_coworker_ceiling`, `switch_ceiling_tool`): a switch flips at once,
//! the whole ceiling is PUT with the version it was read at, and a stale
//! (409), unknown (no answer) or refused switch reads the ceiling again.
//! A broker without the route (501 `not_ported`, or 404) is a quiet state,
//! never a toast.

use std::sync::atomic::{AtomicU64, Ordering};

use nsh_core::prelude::*;
use serde_json::{json, Value};

use crate::client;
use crate::live::store;

/// The card's line while the broker has no `/coworkers/{id}/ceiling` yet.
pub const NEEDS_NEWER_BROKER: &str = "Per-Bot tools need a newer broker";
/// The card's line when the broker did not answer the read.
pub const BROKER_OFFLINE: &str = "Could not reach the broker to read this Bot's tools.";
/// The card's line while the first read is out.
pub const READING: &str = "Reading this Bot's tools…";
/// `NOTHING_TO_SWITCH` (opengrok `src/components/agent_settings.rs`).
pub const NOTHING_TO_SWITCH: &str = "Nothing to switch on or off.";
/// `NO_MAC_RUNS_COMMANDS` (opengrok `src/components/agent_settings.rs`).
pub const NO_MAC_RUNS_COMMANDS: &str = "Not available until this Mac runs commands for Bots";
/// `NOT_AVAILABLE_NOW` (opengrok `src/components/agent_settings.rs`).
pub const NOT_AVAILABLE_NOW: &str = "Not available on this server now.";
/// `NO_LONGER_ON_THE_SERVER` (opengrok `src/components/agent_settings.rs`).
pub const NO_LONGER_ON_THE_SERVER: &str = "No longer on the server";
/// The note after a 409: the rows were replaced by the broker's.
pub const CHANGED_SINCE: &str = "Someone changed this Bot's tools since you looked; here they are now.";
/// The note when the PUT got no answer: the switch may or may not have landed.
pub const NO_ANSWER: &str = "The broker did not answer, so this may not have changed.";
/// `USER_MACHINE_SHELL` (opengrok `src/opengrok/client.rs`): the row for the
/// person's own machine.
pub const USER_MACHINE_SHELL: &str = "user_machine_shell";

/// `CeilingKind` (opengrok `src/opengrok/client.rs`): "builtin" for the
/// broker's own tools; anything else, including kinds this app does not know
/// yet, is a plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CeilingKind {
    Builtin,
    Plugin,
}

impl CeilingKind {
    fn parse(v: &Value) -> Self {
        match v.as_str() {
            Some("builtin") => Self::Builtin,
            _ => Self::Plugin,
        }
    }

    /// The group heading NativeChat's Tools card puts over rows of this kind.
    pub fn heading(self) -> &'static str {
        match self {
            Self::Builtin => "Built in",
            Self::Plugin => "Plugins",
        }
    }
}

/// `CeilingRow` (opengrok `src/opengrok/client.rs`). `raw` is the row as the
/// broker sent it, fields this app does not read included.
#[derive(Clone, Debug, PartialEq)]
pub struct CeilingRow {
    pub name: String,
    pub kind: CeilingKind,
    pub enabled: bool,
    /// Sent only where it can be false; absent means available.
    pub available: Option<bool>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub connector: Option<String>,
    pub summary: Option<String>,
    pub raw: Value,
}

fn opt_str(v: &Value, k: &str) -> Option<String> {
    v[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

impl CeilingRow {
    pub fn from_json(v: &Value) -> Option<Self> {
        let name = v["name"].as_str()?.to_owned();
        Some(Self {
            name,
            kind: CeilingKind::parse(&v["kind"]),
            enabled: v["enabled"].as_bool().unwrap_or(false),
            available: v["available"].as_bool(),
            label: opt_str(v, "label"),
            description: opt_str(v, "description"),
            connector: opt_str(v, "connector"),
            summary: opt_str(v, "summary"),
            raw: v.clone(),
        })
    }

    pub fn is_builtin(&self) -> bool {
        self.kind == CeilingKind::Builtin
    }

    /// `CeilingRow::is_available`: only an explicit `false` is unavailable.
    pub fn is_available(&self) -> bool {
        self.available != Some(false)
    }

    /// `ceiling_title` (opengrok `src/components/agent_settings.rs`).
    pub fn title(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }

    /// `unavailable_why` (opengrok `src/components/agent_settings.rs`): why
    /// the broker could not offer the row, when it could not.
    pub fn unavailable_why(&self) -> Option<&'static str> {
        if self.is_available() {
            return None;
        }
        Some(if self.name == USER_MACHINE_SHELL {
            NO_MAC_RUNS_COMMANDS
        } else if self.is_builtin() {
            NOT_AVAILABLE_NOW
        } else {
            NO_LONGER_ON_THE_SERVER
        })
    }

    /// `connector_note` (opengrok `src/components/agent_settings.rs`): the
    /// plugin works through one of the person's connections.
    pub fn connector_note(&self) -> Option<String> {
        if self.is_builtin() {
            return None;
        }
        let connector = self.connector.as_deref()?;
        let named = match self.label.as_deref() {
            Some(label) if connector == self.name => label,
            _ => connector,
        };
        Some(format!("Uses your {named} connection"))
    }

    /// `first_line_of` (opengrok `src/components/agent_settings.rs`): the
    /// description's first line, cut at a word before 160 characters.
    pub fn blurb(&self) -> Option<String> {
        const MOST: usize = 160;
        let text = self.description.as_deref().or(self.summary.as_deref())?;
        let line = text.lines().next().unwrap_or_default().trim();
        if line.is_empty() {
            return None;
        }
        if line.chars().count() <= MOST {
            return Some(line.to_owned());
        }
        let cut: String = line.chars().take(MOST).collect();
        let at = cut.rfind(' ').unwrap_or(cut.len());
        Some(format!("{}…", cut[..at].trim_end()))
    }

    /// The line under the row's title: why it cannot move, else whose
    /// connection it uses, else what it does.
    pub fn subtitle(&self) -> Option<String> {
        self.unavailable_why().map(str::to_owned).or_else(|| self.connector_note()).or_else(|| self.blurb())
    }
}

/// `CoworkerCeiling` (opengrok `src/opengrok/client.rs`): the rows and the
/// version they were read at, which the next PUT carries.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CoworkerCeiling {
    pub tools: Vec<CeilingRow>,
    pub version: Option<i64>,
}

impl CoworkerCeiling {
    /// `None` when the body is not a ceiling (no `tools` array).
    pub fn from_json(v: &Value) -> Option<Self> {
        let tools = v["tools"].as_array()?.iter().filter_map(CeilingRow::from_json).collect();
        Some(Self { tools, version: v["version"].as_i64() })
    }

    /// The rows of one kind, in the broker's order.
    pub fn of_kind(&self, kind: CeilingKind) -> Vec<CeilingRow> {
        self.tools.iter().filter(|r| r.kind == kind).cloned().collect()
    }
}

/// Why the card has no switches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// 501 `not_ported` or 404: the broker has no ceiling route yet.
    NotPorted,
    /// No answer from the broker.
    Offline,
    /// Any other refusal, in the broker's words.
    Refused(String),
}

impl Unavailable {
    pub fn words(&self) -> &str {
        match self {
            Self::NotPorted => NEEDS_NEWER_BROKER,
            Self::Offline => BROKER_OFFLINE,
            Self::Refused(words) => words,
        }
    }
}

/// `ToolCeiling` (opengrok `src/state.rs`).
#[derive(Clone, Debug, PartialEq)]
pub enum ToolCeiling {
    Loading,
    Read(CoworkerCeiling),
    Unavailable(Unavailable),
}

/// `CeilingCard` (opengrok `src/state.rs`): the card for one bot. `pending`
/// is the switch with the broker, drawn at its target until the answer;
/// no other switch is sent meanwhile, since each PUT carries the whole
/// ceiling. `note` is a line about the last switch that did not take.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CeilingCard {
    pub bot: String,
    pub ceiling: Option<ToolCeiling>,
    pub pending: Option<(String, bool)>,
    pub reading: bool,
    pub note: Option<String>,
}

impl CeilingCard {
    pub fn rows(&self) -> Option<&CoworkerCeiling> {
        match &self.ceiling {
            Some(ToolCeiling::Read(read)) => Some(read),
            _ => None,
        }
    }

    /// Where a row's switch stands on screen: its target while it is with
    /// the broker, else what the broker said.
    pub fn shown_on(&self, row: &CeilingRow) -> bool {
        match &self.pending {
            Some((name, on)) if *name == row.name => *on,
            _ => row.enabled,
        }
    }

    /// Whether the row's switch moves: not while another is with the broker,
    /// and an unavailable row only goes off.
    pub fn may_switch(&self, row: &CeilingRow) -> bool {
        self.pending.is_none() && (row.is_available() || self.shown_on(row))
    }

    /// `ceiling_line` (opengrok `src/components/agent_settings.rs`): the
    /// card's one line when it has no rows to show.
    pub fn line(&self) -> Option<String> {
        match &self.ceiling {
            None => None,
            Some(ToolCeiling::Loading) => Some(READING.to_owned()),
            Some(ToolCeiling::Unavailable(why)) => Some(why.words().to_owned()),
            Some(ToolCeiling::Read(read)) if read.tools.is_empty() => Some(NOTHING_TO_SWITCH.to_owned()),
            Some(ToolCeiling::Read(_)) => None,
        }
    }
}

/// The newest read's number; an older read's answer is dropped
/// (`refresh_coworker_ceiling`'s generation).
static READ: AtomicU64 = AtomicU64::new(0);

fn route(bot: &str) -> String {
    format!("/coworkers/{bot}/ceiling")
}

/// How a read's answer lands on the card.
fn settle(got: Option<(u16, Value)>) -> ToolCeiling {
    match got {
        None => ToolCeiling::Unavailable(Unavailable::Offline),
        Some((404 | 501, _)) => ToolCeiling::Unavailable(Unavailable::NotPorted),
        Some((status, v)) if (200..300).contains(&status) => match CoworkerCeiling::from_json(&v) {
            Some(read) => ToolCeiling::Read(read),
            None => ToolCeiling::Unavailable(Unavailable::Refused("The broker's answer was not a list of tools.".to_owned())),
        },
        Some((_, v)) => ToolCeiling::Unavailable(Unavailable::Refused(refusal(&v))),
    }
}

/// The broker's words in a refusal (`{"error"}` or `{"code"}`), or a plain one.
fn refusal(v: &Value) -> String {
    v["error"]
        .as_str()
        .or_else(|| v["message"].as_str())
        .or_else(|| v["code"].as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| "The broker would not read this Bot's tools.".to_owned())
}

/// `refresh_coworker_ceiling`: read `bot`'s ceiling. A bot the card already
/// shows keeps its rows while the read is out; another bot starts Loading.
/// An empty `bot` clears the card.
pub fn load(bot: String) {
    let st = store();
    if bot.is_empty() {
        READ.fetch_add(1, Ordering::Relaxed);
        st.ceiling.set(CeilingCard::default());
        return;
    }
    st.ceiling.update(|c| {
        if c.bot != bot {
            *c = CeilingCard { bot: bot.clone(), ceiling: Some(ToolCeiling::Loading), ..Default::default() };
        } else if c.rows().is_none() {
            c.ceiling = Some(ToolCeiling::Loading);
        }
        c.reading = true;
    });
    let read = READ.fetch_add(1, Ordering::Relaxed) + 1;
    // Detached: the card's own scope is torn down by the very signal write
    // the answer makes (and by the pane's re-renders), which would cancel a
    // task owned by it (`routines.rs` does the same).
    Owner::detached().run(|| {
        spawn(async move {
            let got = client::call("GET", &route(&bot), None).await;
            if READ.load(Ordering::Relaxed) != read {
                return;
            }
            st.ceiling.update(|c| {
                if c.bot != bot {
                    return;
                }
                c.reading = false;
                c.ceiling = Some(settle(got));
            });
        });
    });
}

/// `switch_ceiling_tool`: flip `name` to `on` for the bot on the card. The
/// row is drawn at `on` at once; the whole ceiling is PUT with its version;
/// the answer replaces the rows. 409 (stale), no answer, or a refusal reads
/// the ceiling again, with a note; 501/404 turns the card quiet.
pub fn switch(name: String, on: bool) {
    let st = store();
    let Some((bot, enabled, version)) = st.ceiling.with(|c| {
        let read = c.rows()?;
        let row = read.tools.iter().find(|r| r.name == name)?;
        if !c.may_switch(row) || row.enabled == on {
            return None;
        }
        let enabled: Vec<String> = read
            .tools
            .iter()
            .filter(|r| if r.name == name { on } else { r.enabled })
            .map(|r| r.name.clone())
            .collect();
        Some((c.bot.clone(), enabled, read.version))
    }) else {
        return;
    };
    st.ceiling.update(|c| {
        c.pending = Some((name.clone(), on));
        c.note = None;
    });
    // Detached, as in `load`: writing `pending` re-renders the row that
    // pressed, and a task it owned would be cancelled with it.
    Owner::detached().run(|| {
        spawn(async move {
            let mut body = json!({ "enabled": enabled });
            if let Some(version) = version {
                body["version"] = json!(version);
            }
            let got = client::call("PUT", &route(&bot), Some(body)).await;
            let mut again = false;
            st.ceiling.update(|c| {
                if c.bot != bot || c.pending.as_ref().is_none_or(|(n, _)| *n != name) {
                    return;
                }
                c.pending = None;
                match got {
                    Some((status, v)) if (200..300).contains(&status) => match CoworkerCeiling::from_json(&v) {
                        Some(read) => c.ceiling = Some(ToolCeiling::Read(read)),
                        None => again = true,
                    },
                    Some((409, _)) => {
                        c.note = Some(CHANGED_SINCE.to_owned());
                        again = true;
                    }
                    Some((404 | 501, _)) => c.ceiling = Some(ToolCeiling::Unavailable(Unavailable::NotPorted)),
                    None => {
                        c.note = Some(NO_ANSWER.to_owned());
                        again = true;
                    }
                    Some((_, v)) => {
                        c.note = Some(refusal(&v));
                        again = true;
                    }
                }
            });
            if again {
                load(bot);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        json!({
            "tools": [
                {"name": "shell_run", "kind": "builtin", "description": "Run a command\nmore", "enabled": true},
                {"name": "user_machine_shell", "kind": "builtin", "description": "Your Mac", "enabled": false, "available": false},
                {"name": "gh", "kind": "plugin", "label": "GitHub", "description": "", "enabled": true, "connector": "gh", "extra": {"x": 1}},
                {"name": "old", "kind": "plugin", "enabled": true, "available": false},
                {"name": "odd", "kind": "something_new", "enabled": false}
            ],
            "version": 7
        })
    }

    #[test]
    fn reads_the_wire_shape() {
        let read = CoworkerCeiling::from_json(&fixture()).unwrap();
        assert_eq!(read.version, Some(7));
        assert_eq!(read.tools.len(), 5);
        assert_eq!(read.of_kind(CeilingKind::Builtin).len(), 2);
        assert_eq!(read.of_kind(CeilingKind::Plugin).len(), 3);
        assert_eq!(read.tools[4].kind, CeilingKind::Plugin);
        assert_eq!(read.tools[2].raw["extra"]["x"], 1);
        assert!(CoworkerCeiling::from_json(&json!({"ok": false})).is_none());
    }

    #[test]
    fn row_words() {
        let read = CoworkerCeiling::from_json(&fixture()).unwrap();
        assert_eq!(read.tools[0].subtitle().as_deref(), Some("Run a command"));
        assert_eq!(read.tools[1].subtitle().as_deref(), Some(NO_MAC_RUNS_COMMANDS));
        assert_eq!(read.tools[2].title(), "GitHub");
        assert_eq!(read.tools[2].subtitle().as_deref(), Some("Uses your GitHub connection"));
        assert_eq!(read.tools[3].subtitle().as_deref(), Some(NO_LONGER_ON_THE_SERVER));
        assert_eq!(read.tools[4].subtitle(), None);
    }

    #[test]
    fn settles_each_answer() {
        assert_eq!(settle(None), ToolCeiling::Unavailable(Unavailable::Offline));
        assert_eq!(settle(Some((501, json!({"ok": false, "code": "not_ported"})))), ToolCeiling::Unavailable(Unavailable::NotPorted));
        assert_eq!(settle(Some((404, Value::Null))), ToolCeiling::Unavailable(Unavailable::NotPorted));
        assert_eq!(settle(Some((403, json!({"error": "not yours"})))), ToolCeiling::Unavailable(Unavailable::Refused("not yours".into())));
        assert!(matches!(settle(Some((200, fixture()))), ToolCeiling::Read(_)));
        assert!(matches!(settle(Some((200, json!("nope")))), ToolCeiling::Unavailable(Unavailable::Refused(_))));
    }

    #[test]
    fn pending_switch_is_drawn_and_blocks_the_rest() {
        let read = CoworkerCeiling::from_json(&fixture()).unwrap();
        let mut card = CeilingCard { bot: "b".into(), ceiling: Some(ToolCeiling::Read(read.clone())), ..Default::default() };
        assert!(card.may_switch(&read.tools[0]));
        assert!(!card.may_switch(&read.tools[1]), "an unavailable row that is off stays off");
        assert!(card.may_switch(&read.tools[3]), "an unavailable row that is on may go off");
        card.pending = Some(("shell_run".into(), false));
        assert!(!card.shown_on(&read.tools[0]));
        assert!(card.shown_on(&read.tools[2]));
        assert!(!card.may_switch(&read.tools[2]));
        assert_eq!(card.line(), None);
        card.ceiling = Some(ToolCeiling::Unavailable(Unavailable::NotPorted));
        assert_eq!(card.line().as_deref(), Some(NEEDS_NEWER_BROKER));
    }
}
