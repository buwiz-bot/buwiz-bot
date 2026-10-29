//! Agents messaging each other (drafts/agent-messaging.md §4.1, §6): the
//! broker's payloads as Rox reads them, and the lines it draws from them.
//!
//! An agents' conversation lives in its own thread (`agents` on the
//! `/threads` row): never a sidebar row nor a ⌘1–9 slot, never restored as
//! the open chat, and read-only (no composer). Each participant's chat gets
//! a centered notice that opens it. A temporary helper (`helper` on the
//! bot) is no sidebar row either: its parent's chat and Details pane reach
//! it. A permanent agent another one made (`made_by`) is listed, marked
//! "Made by <parent>".
//!
//! The views are with their kind: the bubbles and notices in `chat.rs`,
//! the header pill in `hot.rs`, the read-only line in `composer.rs`, the
//! Details sections in `pane.rs`.

use crate::store::{RoxBot, ThreadMeta};

fn s(v: &serde_json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_owned()
}

fn f(v: &serde_json::Value, k: &str) -> f64 {
    v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0)
}

/// The `agents` field of a `/threads` row: whose conversation it is.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct AgentsOf {
    pub conversation: String,
    /// Bot ids, the starter first.
    pub participants: Vec<String>,
    pub subject: String,
    pub created_by: String,
    /// Epoch ms.
    pub created_at: f64,
    pub archived: bool,
    /// Paused by the hourly cap since (epoch ms); 0 while it runs.
    pub paused: f64,
}

impl AgentsOf {
    pub fn of(v: &serde_json::Value) -> Option<Self> {
        let a = Self {
            conversation: s(v, "conversation"),
            participants: v
                .get("participants")
                .and_then(|p| p.as_array())
                .map(|p| p.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
                .unwrap_or_default(),
            subject: s(v, "subject"),
            created_by: s(v, "created_by"),
            created_at: f(v, "created_at"),
            archived: v.get("archived").and_then(|x| x.as_bool()).unwrap_or(false),
            paused: f(v, "paused"),
        };
        (!a.conversation.is_empty()).then_some(a)
    }
}

/// A temporary helper's `helper` field: the agent that made it, its task,
/// and when it ended (0 while it is live).
#[derive(Clone, PartialEq, Default, Debug)]
pub struct HelperOf {
    pub parent: String,
    pub task: String,
    pub created_at: f64,
    pub ended_at: f64,
}

impl HelperOf {
    pub fn of(v: &serde_json::Value) -> Option<Self> {
        let h = Self { parent: s(v, "parent"), task: s(v, "task"), created_at: f(v, "created_at"), ended_at: f(v, "ended_at") };
        (!h.parent.is_empty()).then_some(h)
    }
}

/// Whom an agent takes messages from (`accepts`): `all` (the default),
/// `none`, or `some` for a list of agents (set by the agent tools; shown,
/// not edited, here).
pub fn accepts_of(v: &serde_json::Value) -> String {
    match v.get("accepts") {
        Some(serde_json::Value::String(x)) if x == "none" => "none".to_owned(),
        Some(serde_json::Value::Array(_)) => "some".to_owned(),
        _ => "all".to_owned(),
    }
}

/// An agent on an envelope (`from`, `to`).
#[derive(Clone, PartialEq, Default, Debug)]
pub struct Peer {
    pub id: String,
    pub name: String,
}

impl Peer {
    fn of(v: &serde_json::Value) -> Self {
        let id = s(v, "id");
        let name = Some(s(v, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
        Self { id, name }
    }
}

/// An `agent_message` turn's envelope (§3).
#[derive(Clone, PartialEq, Default, Debug)]
pub struct AgentMessage {
    pub id: String,
    pub in_reply_to: String,
    pub from: Peer,
    pub to: Vec<Peer>,
    pub subject: String,
    pub text: String,
    /// `data`, pretty-printed ("" without one): collapsed under Details.
    pub data: String,
    /// `@plugin:scope:tkt_…` tickets: chips.
    pub attachments: Vec<String>,
}

impl AgentMessage {
    pub fn of(v: &serde_json::Value) -> Option<Self> {
        if !v.is_object() || s(v, "id").is_empty() {
            return None;
        }
        let data = match v.get("data") {
            None | Some(serde_json::Value::Null) => String::new(),
            Some(d) if d.as_object().is_some_and(|o| o.is_empty()) => String::new(),
            Some(d) => serde_json::to_string_pretty(d).unwrap_or_default(),
        };
        Some(Self {
            id: s(v, "id"),
            in_reply_to: s(v, "in_reply_to"),
            from: Peer::of(v.get("from").unwrap_or(&serde_json::Value::Null)),
            to: v.get("to").and_then(|t| t.as_array()).map(|t| t.iter().map(Peer::of).collect()).unwrap_or_default(),
            subject: s(v, "subject"),
            text: s(v, "text"),
            data,
            attachments: v
                .get("attachments")
                .and_then(|a| a.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
                .unwrap_or_default(),
        })
    }

    /// The line over its bubble: "Piccolo → Boss", "Piccolo → 3 agents".
    pub fn header(&self) -> String {
        match self.to.as_slice() {
            [] => self.from.name.clone(),
            [one] => format!("{} → {}", self.from.name, one.name),
            many => format!("{} → {} agents", self.from.name, many.len()),
        }
    }
}

/// The kinds of agents' notice a chat or conversation shows.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum NoticeKind {
    /// Messages in a conversation this chat's agent is in (Open).
    #[default]
    Agents,
    /// This agent made another (Open: its chat).
    Created,
    /// Its helper ended (Open: the helper's chat).
    HelperFinished,
    /// The hourly cap paused a conversation (Resume, Open).
    Paused,
    /// A message relayed too many times was refused (plain).
    HopLimit,
    /// A recipient waits for the user's OK in its own chat (Open: that
    /// chat).
    Waiting,
    /// A message to an outside endpoint failed (plain).
    Undelivered,
}

/// A `notice` turn about agents: its line (the turn's text, as the broker
/// wrote it) and where its actions go.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct AgentsNotice {
    pub kind: NoticeKind,
    pub text: String,
    /// The thread Open goes to (its uid when the notice has one).
    pub thread: String,
    /// The message to scroll to there ("" for none).
    pub message: String,
    pub conversation: String,
}

impl AgentsNotice {
    pub fn of(v: &serde_json::Value, text: String) -> Option<Self> {
        let kind = match s(v, "kind").as_str() {
            "agents" => NoticeKind::Agents,
            "agent_created" => NoticeKind::Created,
            "helper_finished" => NoticeKind::HelperFinished,
            "agents_paused" => NoticeKind::Paused,
            "agents_hop_limit" => NoticeKind::HopLimit,
            "agents_waiting" => NoticeKind::Waiting,
            "agents_undelivered" => NoticeKind::Undelivered,
            _ => return None,
        };
        let thread = Some(s(v, "thread_uid")).filter(|u| !u.is_empty()).unwrap_or_else(|| s(v, "thread"));
        Some(Self { kind, text, thread, message: s(v, "last_id"), conversation: s(v, "conversation") })
    }

    /// Whether it has an Open (a thread to go to).
    pub fn opens(&self) -> bool {
        !self.thread.is_empty() && !matches!(self.kind, NoticeKind::HopLimit | NoticeKind::Undelivered)
    }

    /// The Open action's accessible name: "Open Q3 payments", "Open Sorter".
    pub fn open_label(&self) -> String {
        let what = self.text.split(" · ").nth(1).filter(|_| self.kind == NoticeKind::Agents);
        match (self.kind, what) {
            (NoticeKind::Agents, Some(subject)) => format!("Open {subject}"),
            (NoticeKind::Created | NoticeKind::HelperFinished, _) => {
                // "Piccolo made a new agent: Watcher", "Helper Sorter finished"
                let name = self
                    .text
                    .split(": ")
                    .nth(1)
                    .map(|r| r.split(" · ").next().unwrap_or(r).to_owned())
                    .or_else(|| self.text.strip_prefix("Helper ").and_then(|r| r.strip_suffix(" finished")).map(str::to_owned))
                    .unwrap_or_default();
                if name.is_empty() { "Open its chat".to_owned() } else { format!("Open {name}'s chat") }
            }
            (NoticeKind::Waiting, _) => {
                // "Waiting for your OK in Boss's chat"
                let name = self.text.split(" in ").nth(1).and_then(|r| r.strip_suffix("'s chat")).unwrap_or_default();
                if name.is_empty() { "Open its chat".to_owned() } else { format!("Open {name}'s chat") }
            }
            _ => "Open the conversation".to_owned(),
        }
    }
}

/// The `agent` mark on a turn in an agent's own chat: on a user turn, the
/// reply from another agent that woke it; on an answer (`ai`, with its
/// cards), the work it did for another agent's request.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct ReplyFrom {
    pub name: String,
    /// On an answer: "For <name>'s request".
    pub request: bool,
    /// The conversation's thread (its uid when given).
    pub thread: String,
    /// The message there.
    pub message: String,
}

impl ReplyFrom {
    pub fn of(v: &serde_json::Value, request: bool) -> Option<Self> {
        if !v.is_object() {
            return None;
        }
        let from = v.get("from").map(Peer::of).unwrap_or_default();
        let thread = Some(s(v, "thread_uid")).filter(|u| !u.is_empty()).unwrap_or_else(|| s(v, "thread"));
        (!thread.is_empty() || !from.name.is_empty()).then(|| Self {
            name: if from.name.is_empty() { "an agent".to_owned() } else { from.name },
            request,
            thread,
            message: s(v, "message"),
        })
    }

    /// "Reply from Boss", or "For Piccolo's request" on an answer.
    pub fn caption(&self) -> String {
        if self.request { format!("For {}'s request", self.name) } else { format!("Reply from {}", self.name) }
    }

    /// The turn's text under its caption: the broker leads it with the
    /// caption ("Reply from Boss · Q3 payments"), which isn't said twice.
    pub fn body(&self, text: &str) -> String {
        let cap = self.caption();
        match text.strip_prefix(&cap) {
            Some(rest) => rest.trim_start_matches(" · ").trim_start().to_owned(),
            None => text.to_owned(),
        }
    }
}

/// Whether thread `t` is a sidebar chat row (and a ⌘1–9 slot): not a
/// bot's, a routine's nor an agents' conversation.
pub fn listed_chat(t: &ThreadMeta) -> bool {
    t.bot.is_none() && t.routine.is_none() && t.agents.is_none()
}

/// Whether bot `b` is a sidebar row (and a slot): any but a temporary
/// helper.
pub fn listed_bot(b: &RoxBot) -> bool {
    b.helper.is_none()
}

/// Bot `id`'s name, or the id when it is gone. An outside endpoint
/// (`ext:…`) has no bot: "External".
pub fn name_of(id: &str, bots: &[RoxBot]) -> String {
    bots.iter()
        .find(|b| b.id == id)
        .map(|b| b.name.clone())
        .unwrap_or_else(|| if is_external(id) { "External".to_owned() } else { id.to_owned() })
}

/// Whether participant `id` is an outside endpoint (`ext:ah_…`), not a bot.
pub fn is_external(id: &str) -> bool {
    id.starts_with("ext:")
}

/// A participant's name: its bot's, else the name its messages carry (an
/// outside endpoint's "CI"), else [`name_of`].
pub fn participant_name(id: &str, bots: &[RoxBot], seen: &[Peer]) -> String {
    if let Some(b) = bots.iter().find(|b| b.id == id) {
        return b.name.clone();
    }
    seen.iter().find(|p| p.id == id && p.name != p.id).map(|p| p.name.clone()).unwrap_or_else(|| name_of(id, bots))
}

/// Every sender and recipient the messages name, for [`participant_name`].
pub fn peers_in(messages: impl IntoIterator<Item = AgentMessage>) -> Vec<Peer> {
    let mut out: Vec<Peer> = Vec::new();
    for m in messages {
        for p in std::iter::once(m.from).chain(m.to) {
            if !out.iter().any(|q| q.id == p.id) {
                out.push(p);
            }
        }
    }
    out
}

/// A neutral avatar's letters for a participant with no bot: "CI",
/// "Claude Code · rox" → "CC".
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    match words.as_slice() {
        [] => "?".to_owned(),
        [one] if one.len() <= 2 && one.chars().all(|c| c.is_uppercase()) => one.to_string(),
        [one] => one.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default(),
        [a, b, ..] => [a, b].iter().filter_map(|w| w.chars().next()).flat_map(char::to_uppercase).collect(),
    }
}

/// The header pill's words: "Piccolo ↔ Boss", "+N" past two.
pub fn pill_text(names: &[String]) -> String {
    match names {
        [] => "Agents".to_owned(),
        [one] => one.clone(),
        [a, b] => format!("{a} ↔ {b}"),
        [a, b, rest @ ..] => format!("{a} ↔ {b} +{}", rest.len()),
    }
}

/// The quiet line in place of the composer.
pub fn read_only_line(name: &str) -> String {
    format!("Agents only · read-only · Message {name} in its chat")
}

/// Whom the read-only line sends the user to: the starter, else the first
/// participant.
pub fn read_only_target(a: &AgentsOf) -> String {
    if a.created_by.is_empty() { a.participants.first().cloned().unwrap_or_default() } else { a.created_by.clone() }
}

/// "Made by Piccolo" for an agent another one made; None otherwise.
pub fn made_by_line(b: &RoxBot, bots: &[RoxBot]) -> Option<String> {
    let parent = b.helper.as_ref().map(|h| h.parent.as_str()).filter(|p| !p.is_empty()).unwrap_or(&b.made_by);
    (!parent.is_empty()).then(|| format!("Made by {}", name_of(parent, bots)))
}

/// A sidebar row's subtitle: its preview, after "Made by <parent>" for an
/// agent another one made.
pub fn subtitle(b: &RoxBot, bots: &[RoxBot]) -> String {
    match made_by_line(b, bots) {
        Some(m) if b.desc.is_empty() || b.desc == "No messages yet" => m,
        Some(m) => format!("{m} · {}", b.desc),
        None => b.desc.clone(),
    }
}

/// The thread to open at launch for the server's `active`: never an
/// agents' conversation (its starter's chat instead, else the first
/// listed row).
pub fn restore_target(active: &str, rows: &[ThreadMeta], bots: &[RoxBot]) -> String {
    let Some(a) = rows.iter().find(|t| t.id == active).and_then(|t| t.agents.as_ref()) else {
        return active.to_owned();
    };
    std::iter::once(&a.created_by)
        .chain(&a.participants)
        .find_map(|id| bots.iter().find(|b| b.id == *id).map(|b| b.thread_id.clone()))
        .filter(|t| !t.is_empty())
        .or_else(|| rows.iter().find(|t| t.agents.is_none()).map(|t| t.id.clone()))
        .unwrap_or_default()
}

/// One of a bot's conversations, as its Details pane lists it.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct ConversationRow {
    pub thread: String,
    pub conversation: String,
    pub subject: String,
    /// "Piccolo, Boss".
    pub participants: String,
    pub archived: bool,
    pub paused: bool,
    pub created_at: f64,
}

/// Bot `bot`'s conversations, newest first; archived ones after.
pub fn conversations_of(bot: &str, threads: &[ThreadMeta], bots: &[RoxBot]) -> Vec<ConversationRow> {
    let mut out: Vec<ConversationRow> = threads
        .iter()
        .filter_map(|t| t.agents.as_ref().map(|a| (t, a)))
        .filter(|(_, a)| a.participants.iter().any(|p| p == bot))
        .map(|(t, a)| ConversationRow {
            thread: t.id.clone(),
            conversation: a.conversation.clone(),
            subject: if a.subject.is_empty() { t.name.clone() } else { a.subject.clone() },
            participants: a.participants.iter().map(|p| name_of(p, bots)).collect::<Vec<_>>().join(", "),
            archived: a.archived,
            paused: a.paused > 0.0,
            created_at: a.created_at,
        })
        .collect();
    out.sort_by(|x, y| x.archived.cmp(&y.archived).then(y.created_at.total_cmp(&x.created_at)));
    out
}

/// Bot `parent`'s temporary helpers: (live ones, past ones), newest first.
pub fn helpers_of(parent: &str, bots: &[RoxBot]) -> (Vec<RoxBot>, Vec<RoxBot>) {
    let mut mine: Vec<RoxBot> = bots.iter().filter(|b| b.helper.as_ref().is_some_and(|h| h.parent == parent)).cloned().collect();
    let at = |b: &RoxBot| b.helper.as_ref().map(|h| h.created_at).unwrap_or(0.0);
    mine.sort_by(|x, y| at(y).total_cmp(&at(x)));
    mine.into_iter().partition(|b| b.helper.as_ref().is_some_and(|h| h.ended_at <= 0.0))
}

/// The last message of a conversation's turns and how many there are.
pub fn last_message(turns: &serde_json::Value) -> (i64, String) {
    let msgs: Vec<&serde_json::Value> = turns
        .get("turns")
        .and_then(|t| t.as_array())
        .map(|t| t.iter().filter(|t| s(t, "role") == "agent_message").collect())
        .unwrap_or_default();
    let last = msgs
        .last()
        .map(|m| {
            let from = m.get("message").and_then(|e| e.get("from")).map(Peer::of).map(|p| p.name).unwrap_or_default();
            let text: String = s(m, "text").split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect();
            if from.is_empty() { text } else { format!("{from}: {text}") }
        })
        .unwrap_or_default();
    (msgs.len() as i64, last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{ThreadBot, Turn};
    use serde_json::json;

    fn bot(id: &str, name: &str, thread: &str) -> RoxBot {
        RoxBot { id: id.into(), name: name.into(), thread_id: thread.into(), desc: "No messages yet".into(), ..Default::default() }
    }

    fn conv(id: &str, participants: &[&str], subject: &str, at: f64) -> ThreadMeta {
        ThreadMeta {
            id: id.into(),
            name: subject.into(),
            agents: Some(AgentsOf {
                conversation: format!("conv_{id}"),
                participants: participants.iter().map(|p| p.to_string()).collect(),
                subject: subject.into(),
                created_by: participants[0].into(),
                created_at: at,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn agents_threads_and_helpers_are_never_sidebar_rows() {
        let chat = ThreadMeta { id: "t1".into(), ..Default::default() };
        let own = ThreadMeta { id: "t2".into(), bot: Some(ThreadBot { id: "piccolo".into(), ..Default::default() }), ..Default::default() };
        let c = conv("t5", &["piccolo", "boss"], "Q3", 1.0);
        assert!(listed_chat(&chat));
        assert!(!listed_chat(&own), "a bot's chat is its bot's row");
        assert!(!listed_chat(&c), "a conversation is no chat row");
        let row = json!({"conversation": "conv_1", "participants": ["piccolo", "boss"], "subject": "Q3", "created_by": "piccolo", "created_at": 5, "paused": 9});
        let a = AgentsOf::of(&row).expect("agents");
        assert_eq!((a.participants.len(), a.paused, a.archived), (2, 9.0, false));
        assert_eq!(AgentsOf::of(&json!({"subject": "x"})), None, "no conversation, no mark");
        let mut sorter = bot("sorter", "Sorter", "t8");
        sorter.helper = HelperOf::of(&json!({"parent": "piccolo", "task": "Sort", "created_at": 3}));
        assert!(!listed_bot(&sorter));
        let mut watcher = bot("watcher", "Watcher", "t7");
        watcher.made_by = "piccolo".into();
        assert!(listed_bot(&watcher), "a permanent agent is listed");
        assert_eq!(HelperOf::of(&json!({"task": "x"})), None);
        assert_eq!(accepts_of(&json!({})), "all");
        assert_eq!(accepts_of(&json!({"accepts": "none"})), "none");
        assert_eq!(accepts_of(&json!({"accepts": ["boss"]})), "some");
    }

    #[test]
    fn an_agents_thread_is_never_restored_as_the_open_chat() {
        let bots = vec![bot("piccolo", "Piccolo", "t2"), bot("boss", "Boss", "t3")];
        let rows = vec![ThreadMeta { id: "t1".into(), ..Default::default() }, conv("t5", &["boss", "piccolo"], "Q3", 1.0)];
        assert_eq!(restore_target("t1", &rows, &bots), "t1");
        assert_eq!(restore_target("t5", &rows, &bots), "t3", "its starter's chat");
        assert_eq!(restore_target("t5", &rows, &[]), "t1", "the first other row when its agents are gone");
        assert_eq!(restore_target("t9", &rows, &bots), "t9", "an unknown id is left to the usual fallback");
    }

    #[test]
    fn agent_message_turns_and_notices_parse() {
        let m = Turn::of(&json!({"i": 0, "id": "u1", "role": "agent_message", "text": "Please check",
            "message": {"v": 1, "id": "msg_1", "conversation": "conv_1", "from": {"kind": "agent", "id": "piccolo", "name": "Piccolo"},
                "to": [{"kind": "agent", "id": "boss", "name": "Boss"}], "subject": "Q3", "text": "Please check",
                "data": {"form": "2551Q"}, "attachments": ["@bir:taxprofile:tkt_01JC9Q4R8M2ZKX7T3N5V6W8Y0A"], "expects_reply": true, "hop": 0}}));
        let e = m.message.expect("an envelope");
        assert_eq!((e.id.as_str(), e.from.name.as_str(), e.header().as_str()), ("msg_1", "Piccolo", "Piccolo → Boss"));
        assert!(e.data.contains("\"form\": \"2551Q\""));
        assert_eq!(e.attachments.len(), 1);
        let broadcast = AgentMessage::of(&json!({"id": "m", "from": {"id": "piccolo"}, "to": [{"id": "a", "name": "A"}, {"id": "b", "name": "B"}]})).unwrap();
        assert_eq!(broadcast.header(), "piccolo → 2 agents", "a nameless sender reads as its id");
        assert_eq!(broadcast.data, "", "no data, no Details");
        assert!(Turn::of(&json!({"role": "ai", "text": "x"})).message.is_none());

        let n = Turn::of(&json!({"role": "notice", "text": "Boss → Piccolo · Q3 payments for a client · 2 messages",
            "notice": {"kind": "agents", "conversation": "conv_1", "thread": "t5", "thread_uid": "uid-5", "last_id": "msg_2", "count": 2}}));
        let n = n.agents_notice.expect("an agents notice");
        assert_eq!((n.kind, n.thread.as_str(), n.message.as_str()), (NoticeKind::Agents, "uid-5", "msg_2"));
        assert_eq!(n.open_label(), "Open Q3 payments for a client");
        let made = AgentsNotice::of(&json!({"kind": "agent_created", "thread": "t8", "temporary": true}), "Piccolo started a helper: Sorter · Sort receipts".into()).unwrap();
        assert_eq!((made.kind, made.thread.as_str(), made.open_label().as_str()), (NoticeKind::Created, "t8", "Open Sorter's chat"));
        let done = AgentsNotice::of(&json!({"kind": "helper_finished", "thread": "t8"}), "Helper Sorter finished".into()).unwrap();
        assert_eq!(done.open_label(), "Open Sorter's chat");
        let paused = AgentsNotice::of(&json!({"kind": "agents_paused", "conversation": "conv_9", "thread": "t10"}), "Paused".into()).unwrap();
        assert_eq!((paused.kind, paused.conversation.as_str()), (NoticeKind::Paused, "conv_9"));
        assert_eq!(AgentsNotice::of(&json!({"kind": "agents_hop_limit"}), "Stopped".into()).map(|n| n.kind), Some(NoticeKind::HopLimit));
        assert_eq!(AgentsNotice::of(&json!({"kind": "routine"}), String::new()), None, "a routine's notice is not ours");
        let wait = AgentsNotice::of(&json!({"kind": "agents_waiting", "bot": "boss", "thread": "t3", "thread_uid": "uid-3"}), "Waiting for your OK in Boss's chat".into()).unwrap();
        assert_eq!((wait.kind, wait.opens(), wait.thread.as_str(), wait.open_label().as_str()), (NoticeKind::Waiting, true, "uid-3", "Open Boss's chat"));
        let lost = AgentsNotice::of(&json!({"kind": "agents_undelivered", "endpoint": "ah_1", "message": "msg_3"}), "Not delivered to CI (HTTP 404)".into()).unwrap();
        assert_eq!((lost.kind, lost.opens()), (NoticeKind::Undelivered, false), "plain text");
    }

    #[test]
    fn a_turn_woken_by_a_reply_is_captioned_with_its_sender() {
        let t = Turn::of(&json!({"role": "user", "text": "Reply from Boss · Q3",
            "agent": {"conversation": "conv_1", "message": "msg_2", "from": {"kind": "agent", "id": "boss", "name": "Boss"}, "thread": "t5", "thread_uid": "uid-5"}}));
        let r = t.reply_from.expect("a reply mark");
        assert_eq!((r.caption().as_str(), r.thread.as_str(), r.message.as_str()), ("Reply from Boss", "uid-5", "msg_2"));
        assert_eq!(r.body("Reply from Boss · Q3 payments"), "Q3 payments", "the caption isn't said twice");
        assert_eq!(r.body("Both are recorded"), "Both are recorded");
        assert!(Turn::of(&json!({"role": "user", "text": "hi"})).reply_from.is_none());
        // an answer with cards, done for another agent's request
        let a = Turn::of(&json!({"role": "ai", "text": "For Piccolo's request · Q3", "links": [],
            "agent": {"conversation": "conv_1", "message": "msg_1", "from": {"kind": "agent", "id": "piccolo", "name": "Piccolo"}, "thread": "t5"}}));
        let r = a.reply_from.expect("a request mark");
        assert_eq!((r.caption().as_str(), r.thread.as_str()), ("For Piccolo's request", "t5"));
        assert!(Turn::of(&json!({"role": "notice", "agent": {"thread": "t5"}})).reply_from.is_none(), "only a user turn or an answer");
    }

    #[test]
    fn outside_endpoints_read_by_the_names_their_messages_carry() {
        let bots = vec![bot("piccolo", "Piccolo", "t2")];
        let m = AgentMessage::of(&json!({"id": "m1", "from": {"kind": "external", "id": "ext:ah_1", "name": "CI"}, "to": [{"id": "piccolo", "name": "Piccolo"}]})).unwrap();
        let seen = peers_in(vec![m]);
        assert_eq!(participant_name("ext:ah_1", &bots, &seen), "CI");
        assert_eq!(participant_name("ext:ah_2", &bots, &seen), "External", "one that hasn't written yet");
        assert_eq!(participant_name("piccolo", &bots, &[]), "Piccolo");
        assert!(is_external("ext:ah_1") && !is_external("piccolo"));
        assert_eq!(initials("CI"), "CI");
        assert_eq!(initials("Claude Code · rox"), "CC");
        assert_eq!(initials("watcher"), "W");
        assert_eq!(initials(""), "?");
    }

    #[test]
    fn the_pill_names_two_participants_then_counts_the_rest() {
        let names = |n: &[&str]| n.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(pill_text(&names(&["Piccolo", "Boss"])), "Piccolo ↔ Boss");
        assert_eq!(pill_text(&names(&["Piccolo", "Boss", "Ada", "Kai"])), "Piccolo ↔ Boss +2");
        assert_eq!(pill_text(&names(&["Piccolo"])), "Piccolo");
        assert_eq!(read_only_line("Piccolo"), "Agents only · read-only · Message Piccolo in its chat");
        let c = conv("t5", &["piccolo", "boss"], "Q3", 1.0);
        assert_eq!(read_only_target(c.agents.as_ref().unwrap()), "piccolo");
    }

    #[test]
    fn made_by_marks_agents_another_one_made() {
        let piccolo = bot("piccolo", "Piccolo", "t2");
        let mut watcher = bot("watcher", "Watcher", "t7");
        watcher.made_by = "piccolo".into();
        let bots = vec![piccolo.clone(), watcher.clone()];
        assert_eq!(made_by_line(&watcher, &bots).as_deref(), Some("Made by Piccolo"));
        assert_eq!(made_by_line(&piccolo, &bots), None);
        assert_eq!(subtitle(&watcher, &bots), "Made by Piccolo");
        watcher.desc = "Checked the inbox".into();
        assert_eq!(subtitle(&watcher, &bots), "Made by Piccolo · Checked the inbox");
        assert_eq!(subtitle(&piccolo, &bots), "No messages yet");
    }

    #[test]
    fn a_bot_lists_its_conversations_and_helpers() {
        let mut bots = vec![bot("piccolo", "Piccolo", "t2"), bot("boss", "Boss", "t3"), bot("ada", "Ada", "t4")];
        let mut old = conv("t9", &["piccolo", "ada"], "Old", 9.0);
        old.agents.as_mut().unwrap().archived = true;
        let threads = vec![conv("t5", &["piccolo", "boss"], "Q3", 1.0), conv("t6", &["boss", "ada"], "Not his", 2.0), old, conv("t7", &["ada", "piccolo", "boss"], "Friday", 3.0)];
        let rows = conversations_of("piccolo", &threads, &bots);
        let got: Vec<(&str, &str, bool)> = rows.iter().map(|r| (r.subject.as_str(), r.participants.as_str(), r.archived)).collect();
        assert_eq!(got, vec![("Friday", "Ada, Piccolo, Boss", false), ("Q3", "Piccolo, Boss", false), ("Old", "Piccolo, Ada", true)]);
        let mut live = bot("sorter", "Sorter", "t8");
        live.helper = Some(HelperOf { parent: "piccolo".into(), task: "Sort".into(), created_at: 5.0, ended_at: 0.0 });
        let mut past = bot("filer", "Filer", "t11");
        past.helper = Some(HelperOf { parent: "piccolo".into(), task: "File".into(), created_at: 1.0, ended_at: 4.0 });
        let mut other = bot("x", "X", "t12");
        other.helper = Some(HelperOf { parent: "boss".into(), ..Default::default() });
        bots.extend([live, past, other]);
        let (now, before) = helpers_of("piccolo", &bots);
        assert_eq!(now.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), vec!["Sorter"]);
        assert_eq!(before.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), vec!["Filer"]);
    }

    #[test]
    fn a_conversations_last_message_reads_with_its_sender() {
        let turns = json!({"turns": [
            {"role": "agent_message", "text": "Please check", "message": {"from": {"id": "piccolo", "name": "Piccolo"}}},
            {"role": "agent_message", "text": "Both are\nrecorded.", "message": {"from": {"id": "boss", "name": "Boss"}}},
            {"role": "notice", "text": "Paused"},
        ]});
        assert_eq!(last_message(&turns), (2, "Boss: Both are recorded.".to_owned()));
        assert_eq!(last_message(&json!({"turns": []})), (0, String::new()));
    }
}
