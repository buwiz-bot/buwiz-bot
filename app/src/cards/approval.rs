//! The permission card (nativechat `render_approval`): a run parked on a
//! `run-awaiting-approval` frame asks before a tool runs. Its title says
//! where the call would act, the lines under it what it would do, and its
//! buttons answer by kind — the local shell's Always / Never keep a rule
//! for its command on this Mac, the egress tunnel's keep the computer's
//! choice, everything else answers the one call. Answered, it collapses to
//! the sentence its kind leaves behind.

use nsh_core::ui::prelude::*;
use serde_json::Value;

use super::transport::{self, Sent};
use super::{Btn, CardState};

/// The fields `POST /ag-ui/runs/{runId}/answer` needs, plus what the card
/// shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ApprovalSpec {
    pub run_id: String,
    pub thread_id: Option<String>,
    pub call_id: String,
    pub tool: String,
    /// The shell command (or the arguments as JSON when there is no
    /// command): the box under the title when nothing else says it.
    pub command: String,
    pub why: String,
    pub reason: String,
    /// What the call would do, in the server's words: its own `summary`
    /// when the frame carries one, else built the way the server builds it.
    pub summary: String,
}

/// The harness tool that runs on the person's own machine.
pub(crate) const USER_MACHINE_SHELL: &str = "user_machine_shell";
const LIST_ROUTINES: &str = "list_routines";
const CREATE_ROUTINE: &str = "create_routine";
const UPDATE_ROUTINE: &str = "update_routine";
const DELETE_ROUTINE: &str = "delete_routine";
const USE_SKILL: &str = "use_skill";

/// The `reason`s that put Review-an-action chrome on a card.
const REVIEW_AN_ACTION_REASONS: &[&str] = &["auto-review", "review-an-action", "computer-action", "egress"];

/// The sentence OpenGrok puts on the egress tunnel's card, matched whole.
pub(crate) const EGRESS_TUNNEL_ASK_REASON: &str =
    "This action would use your network through the egress tunnel. Review it before it runs.";

/// What a later message leaves where an unanswered card was.
pub(crate) const SUPERSEDED_NOTE: &str = "Moved on to your next message.";

/// The person's answer on a permission card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resolution {
    Always,
    AllowOnce,
    Never,
    DenyOnce,
}

impl Resolution {
    fn approved(self) -> bool {
        matches!(self, Resolution::Always | Resolution::AllowOnce)
    }
}

impl ApprovalSpec {
    /// The card a `run-awaiting-approval` frame carries; `None` without a
    /// run and a call to answer.
    pub(crate) fn from_wire(event: &Value) -> Option<Self> {
        let run_id = string_at(event, "runId")?;
        let call_id = string_at(event, "callId")?;
        if run_id.is_empty() || call_id.is_empty() {
            return None;
        }
        let tool = string_at(event, "tool").unwrap_or_else(|| "a tool".to_owned());
        let arguments = event
            .get("arguments")
            .or_else(|| event.get("value").and_then(|v| v.get("arguments")))
            .cloned()
            .unwrap_or(Value::Null);
        let summary = string_at(event, "summary")
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| approval_summary(&tool, &arguments));
        Some(Self {
            run_id,
            thread_id: string_at(event, "threadId").filter(|id| !id.trim().is_empty()),
            call_id,
            command: command_from_args(&arguments),
            why: string_at(event, "why").unwrap_or_default(),
            reason: string_at(event, "reason").unwrap_or_else(|| "exec-consent".to_owned()),
            summary,
            tool,
        })
    }

    pub(crate) fn runs_on_this_mac(&self) -> bool {
        self.tool == USER_MACHINE_SHELL
    }

    /// The card came off the MCP door (an `mcp-…` thread), not a turn.
    fn is_mcp(&self) -> bool {
        self.thread_id.as_deref().is_some_and(|t| t.starts_with("mcp-"))
    }

    fn place(&self) -> &'static str {
        if self.runs_on_this_mac() { "your computer" } else { "its computer" }
    }

    /// Review-an-action chrome (auto-review / the tunnel). Never on an MCP
    /// card: there is no standing policy behind its Always.
    pub(crate) fn is_review_an_action(&self) -> bool {
        !self.is_mcp() && REVIEW_AN_ACTION_REASONS.contains(&self.reason.trim().to_ascii_lowercase().as_str())
    }

    /// The tunnel's own card: its Always / Never set the computer's
    /// standing egress policy.
    pub(crate) fn is_egress_tunnel(&self) -> bool {
        self.is_review_an_action()
            && !self.runs_on_this_mac()
            && (self.reason.trim().eq_ignore_ascii_case("egress") || self.why.trim() == EGRESS_TUNNEL_ASK_REASON)
    }

    /// `PUT …/egress-policy`'s `mode` an answer to the tunnel's card keeps.
    pub(crate) fn egress_mode(&self, r: Resolution) -> Option<&'static str> {
        if !self.is_egress_tunnel() {
            return None;
        }
        match r {
            Resolution::Always => Some("bypass"),
            Resolution::Never => Some("never"),
            _ => None,
        }
    }

    /// The centered line the card leaves once answered, in its kind's words.
    pub(crate) fn outcome(&self, bot: &str, r: Resolution) -> String {
        if self.is_mcp() {
            match r {
                Resolution::AllowOnce | Resolution::Always => format!("{bot} may run {} once.", self.tool),
                _ => format!("{bot} was told no."),
            }
        } else if self.is_egress_tunnel() {
            match r {
                Resolution::Always => format!("{bot} may use your network from its computer."),
                Resolution::Never => format!("{bot} may not use your network from its computer."),
                Resolution::AllowOnce => format!("{bot} may use your network this time."),
                Resolution::DenyOnce => format!("{bot} was not allowed to use your network this time."),
            }
        } else if self.runs_on_this_mac() {
            match r {
                Resolution::Always => format!("{bot} can run this command on your computer without asking."),
                Resolution::Never => format!("{bot} cannot run this command on your computer."),
                once => local_exec_outcome(bot, once, "your computer"),
            }
        } else {
            local_exec_outcome(bot, r, self.place())
        }
    }
}

fn local_exec_outcome(bot: &str, r: Resolution, place: &str) -> String {
    match r {
        Resolution::Always => format!("{bot} can run commands on {place}."),
        Resolution::Never => format!("{bot} cannot run commands on {place}."),
        Resolution::DenyOnce => format!("{bot} was not allowed to run commands on {place}."),
        Resolution::AllowOnce => format!("{bot} can run commands on {place} this time."),
    }
}

/// The card's title: one short question about where the call would act
/// (nativechat `approval_title`).
pub(crate) fn approval_title(spec: &ApprovalSpec, bot: &str, review: bool) -> String {
    if review {
        return "Review an action".to_owned();
    }
    if spec.runs_on_this_mac() {
        return format!("Allow {bot} to run this command on your local computer?");
    }
    if let Some(question) = routine_question(spec) {
        return question.to_owned();
    }
    match spec.tool.as_str() {
        "shell" => format!("Allow {bot} to run a command on its computer?"),
        "read_file" => format!("Allow {bot} to read a file on its computer?"),
        "write_file" => format!("Allow {bot} to write a file on its computer?"),
        LIST_ROUTINES => format!("Allow {bot} to see its routines?"),
        CREATE_ROUTINE => format!("Allow {bot} to make a routine?"),
        UPDATE_ROUTINE => format!("Allow {bot} to change a routine?"),
        DELETE_ROUTINE => format!("Allow {bot} to delete a routine?"),
        tool if tool.matches('.').count() >= 2 => format!("Allow {bot} to use a connected service?"),
        _ => format!("Allow {bot} to use its computer?"),
    }
}

/// A routine delete's `why` is the server's own question naming the
/// routine as stored: that is the card's title.
fn routine_question(spec: &ApprovalSpec) -> Option<&str> {
    let why = spec.why.trim();
    (spec.tool == DELETE_ROUTINE && !why.is_empty()).then_some(why)
}

/// The lines under the title: a review card says why it stopped the call,
/// then what it would do; a consent card only what it would do.
pub(crate) fn card_lines(spec: &ApprovalSpec, review: bool) -> Vec<String> {
    if !review && routine_question(spec).is_some() {
        return Vec::new();
    }
    let why = if review { spec.why.trim() } else { "" };
    [why, spec.summary.as_str()].into_iter().filter(|l| !l.is_empty()).map(str::to_owned).collect()
}

/// The raw command box, only where nothing else says what the call does.
pub(crate) fn arguments_box(spec: &ApprovalSpec) -> Option<String> {
    if !spec.summary.is_empty() {
        return None;
    }
    Some(if spec.command.trim().is_empty() {
        "Command was not included with this request.".to_owned()
    } else {
        spec.command.clone()
    })
}

// ---------------------------------------------------------------------------
// The card
// ---------------------------------------------------------------------------

/// The card for `spec`, by its state in the store: open (buttons), sending,
/// a failed answer (the sentence under the buttons), or answered (the
/// outcome line).
pub(crate) fn card(spec: ApprovalSpec, key: String) -> impl IntoElement {
    let state = {
        let key = key.clone();
        computed(move || super::state_of(&key))
    };
    dynamic(move || {
        let st = state.get();
        match st {
            Some(CardState::Answered(line)) => super::outcome_line(line).into_element(),
            Some(CardState::Superseded) => super::outcome_line(SUPERSEDED_NOTE.to_owned()).into_element(),
            Some(CardState::Dismissed) => super::outcome_line("Dismissed.".to_owned()).into_element(),
            other => open_card(spec.clone(), key.clone(), other).into_element(),
        }
    })
}

fn open_card(spec: ApprovalSpec, key: String, st: Option<CardState>) -> impl IntoElement {
    let bot = super::bot_name();
    let review = spec.is_review_an_action();
    let local = spec.runs_on_this_mac();
    let sending = matches!(st, Some(CardState::Sending));
    let mut body = super::shell(&format!("approval-{key}"))
        .child(
            view()
                .flex_row()
                .items_start()
                .gap_2()
                .child(super::title(approval_title(&spec, &bot, review)).flex_1())
                .child({
                    let spec = spec.clone();
                    super::close_x("Deny this time", move || answer(spec.clone(), Resolution::DenyOnce))
                }),
        );
    for line in card_lines(&spec, review) {
        body = body.child(super::note(line));
    }
    if local {
        body = body.child(super::note(format!(
            "Always allow and Never answer this command from now on, for {bot} and every Bot."
        )));
    }
    if let Some(command) = arguments_box(&spec) {
        body = body.child(super::code_box(command));
    }
    if sending {
        body = body.child(super::note("Sending…".to_owned()));
    } else {
        let mut row = view().flex_row().flex_wrap().justify_end().gap_2().mt_1();
        let button = |label: &'static str, r: Resolution, kind: Btn| {
            let spec = spec.clone();
            super::btn(kind, label, move || answer(spec.clone(), r))
        };
        if local || review {
            row = row.child(button("Always allow", Resolution::Always, Btn::Primary));
        }
        row = row
            .child(button(
                "Allow once",
                Resolution::AllowOnce,
                if local || review { Btn::Secondary } else { Btn::Primary },
            ))
            .child(button("Deny", Resolution::DenyOnce, Btn::Secondary));
        // A standing no: the command's rule on this Mac, or the
        // computer's choice on the tunnel's card.
        if (local && !review) || spec.is_egress_tunnel() {
            row = row.child(button("Never", Resolution::Never, Btn::Secondary));
        }
        body = body.child(row);
    }
    if let Some(CardState::Error(why)) = st {
        body = body.child(super::error_line(why));
    }
    body
}

/// Answer the card: a standing Always / Never writes its rule first (the
/// local shell's, or the tunnel's egress policy), then the run is answered.
/// A rule the server refuses leaves the answer once-only and says so.
pub(crate) fn answer(spec: ApprovalSpec, r: Resolution) {
    let key = spec.call_id.clone();
    if !super::answerable(&key) {
        return;
    }
    super::set_state(&key, CardState::Sending);
    let bot = super::bot_name();
    let coworker = super::agent_id();
    Owner::detached().run(move || {
        spawn(async move {
            let mut offline = false;
            let mut not_kept: Option<String> = None;
            let standing = matches!(r, Resolution::Always | Resolution::Never);
            let standing_word = if r == Resolution::Always { "Always allow" } else { "Never" };
            if standing && spec.runs_on_this_mac() {
                let kind = if r == Resolution::Always { "allow" } else { "deny" };
                match transport::write_rule(kind, &spec.command).await {
                    Sent::Done(_) => {}
                    Sent::Offline => offline = true,
                    Sent::Refused(why) => {
                        not_kept = Some(match transport::plain_reason(&why) {
                            Some(reason) => format!("{standing_word} was not kept: {reason}"),
                            None => format!("{standing_word} was not kept."),
                        })
                    }
                }
            } else if let Some(mode) = spec.egress_mode(r) {
                match transport::write_egress(&coworker, mode).await {
                    Sent::Done(_) => {}
                    Sent::Offline => offline = true,
                    Sent::Refused(why) => {
                        not_kept = Some(if why.trim() == "only the organization's admin may do that" {
                            "Only the organization's admin can make that choice for its shared computer.".to_owned()
                        } else {
                            match transport::plain_reason(&why) {
                                Some(reason) => format!("The choice for every time was not kept: {reason}"),
                                None => "The choice for every time was not kept.".to_owned(),
                            }
                        })
                    }
                }
            }
            let next = match transport::answer_run(&spec.run_id, &spec.call_id, r.approved()).await {
                Sent::Done(_) => None,
                Sent::Offline => {
                    offline = true;
                    None
                }
                Sent::Refused(why) => Some(why),
            };
            let state = match next {
                Some(why) => CardState::Error(why),
                None => match not_kept {
                    Some(why) => {
                        let once = if r.approved() { Resolution::AllowOnce } else { Resolution::DenyOnce };
                        CardState::Answered(format!("{} {why}", spec.outcome(&bot, once)))
                    }
                    None => CardState::Answered(spec.outcome(&bot, r)),
                },
            };
            super::set_state(&key, state);
            if offline {
                transport::not_connected();
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Summaries (nativechat `approval_summary`, the server's `summary_for`)
// ---------------------------------------------------------------------------

/// What a call would do, in the words the server's own card uses. Empty
/// for the shells (their command is the card) and argument-less calls.
pub(crate) fn approval_summary(tool: &str, arguments: &Value) -> String {
    if !arguments.is_object() {
        return String::new();
    }
    match tool {
        USER_MACHINE_SHELL | "shell" => String::new(),
        "read_file" => format!("Read {} on the agent's own box", clip(string_arg(arguments, "path").unwrap_or("a file"), 200)),
        "write_file" => format!(
            "Write {} bytes to {} on the agent's own box",
            string_arg(arguments, "content").map_or(0, str::len),
            clip(string_arg(arguments, "path").unwrap_or("a file"), 200)
        ),
        "computer" => screen_summary(arguments),
        "open_url" => format!("Open {} in the agent's own browser", clip(page_of(arguments), 120)),
        "run_recipe" => format!(
            "Play the recipe \"{}\" on the agent's own computer",
            clip(string_arg(arguments, "recipe").unwrap_or("(unnamed)"), 80)
        ),
        USE_SKILL => format!("Read the skill \"{}\"", clip(string_arg(arguments, "name").unwrap_or("(unnamed)"), 80)),
        LIST_ROUTINES => "List this Bot's routines".to_owned(),
        CREATE_ROUTINE => format!(
            "Make a routine that wakes at {}",
            clip(&arguments.get("when").unwrap_or(&Value::Null).to_string(), 80)
        ),
        UPDATE_ROUTINE | DELETE_ROUTINE => format!(
            "{} the routine {}",
            if tool == DELETE_ROUTINE { "Delete" } else { "Change" },
            clip(string_arg(arguments, "routine").unwrap_or("(none)"), 80)
        ),
        other => format!(
            "{other} — a plugin tool this agent wants to call, with {}",
            clip(&redact_value(arguments, None).to_string(), 160)
        ),
    }
}

fn screen_summary(arguments: &Value) -> String {
    let screen = if string_arg(arguments, "machine") == Some("group") {
        "the group's shared screen"
    } else {
        "the agent's own screen"
    };
    let point = |key: &str| {
        let xy = arguments.get(key).and_then(Value::as_array);
        let at = |i: usize| xy.and_then(|xy| xy.get(i)).and_then(Value::as_i64);
        match (at(0), at(1)) {
            (Some(x), Some(y)) => format!("({x}, {y})"),
            _ => "(no position)".to_owned(),
        }
    };
    let at = point("coordinate");
    match string_arg(arguments, "action").unwrap_or("") {
        "screenshot" => format!("Take a screenshot of {screen}"),
        "click" | "left_click" => format!("Click at {at} on {screen}"),
        "right_click" => format!("Right-click at {at} on {screen}"),
        "double_click" => format!("Double-click at {at} on {screen}"),
        "move" | "mouse_move" => format!("Move the pointer to {at} on {screen}"),
        "drag" | "left_click_drag" => format!("Drag from {at} to {} on {screen}", point("to")),
        "type" => format!("Type \"{}\" on {screen}", shown(string_arg(arguments, "text").unwrap_or(""), 60)),
        "key" => format!("Press {} on {screen}", shown(string_arg(arguments, "key").unwrap_or("a key"), 40)),
        "scroll" => format!("Scroll at {at} on {screen}"),
        other => format!("Screen action \"{}\" on {screen}", clip(other, 30)),
    }
}

fn string_arg<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments.get(key).and_then(Value::as_str)
}

fn shown(text: &str, max: usize) -> String {
    if looks_like_a_secret(text) { REDACTED.to_owned() } else { clip(text, max) }
}

/// A page without its query or fragment, where a link's token rides.
fn page_of(arguments: &Value) -> &str {
    let url = string_arg(arguments, "url").unwrap_or("a page");
    url.split(['?', '#']).next().unwrap_or(url)
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}…")
}

const REDACTED: &str = "«redacted»";

/// The server's coarse test for a key-shaped string.
fn looks_like_a_secret(text: &str) -> bool {
    let t = text.trim();
    if t.starts_with("Bearer ") || t.starts_with("sk-") || t.starts_with("xoxb-") {
        return true;
    }
    t.len() >= 40
        && !t.contains(' ')
        && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '/' | '='))
}

/// A plugin call's arguments as the server's card shows them: identity
/// keys dropped, secrets (by name or by shape) `«redacted»`.
fn redact_value(value: &Value, key: Option<&str>) -> Value {
    const IDENTITY_KEYS: &[&str] = &["account_id", "accountId", "coworker_id", "coworkerId", "box_id", "boxId"];
    const SECRET_KEY_FRAGMENTS: &[&str] =
        &["token", "secret", "password", "passwd", "authorization", "api_key", "apikey", "credential", "cookie"];
    if let Some(key) = key {
        let lower = key.to_ascii_lowercase();
        if SECRET_KEY_FRAGMENTS.iter().any(|f| lower.contains(f)) {
            return Value::String(REDACTED.to_owned());
        }
    }
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| !IDENTITY_KEYS.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), redact_value(v, Some(k))))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|i| redact_value(i, None)).collect()),
        Value::String(t) if looks_like_a_secret(t) => Value::String(REDACTED.to_owned()),
        other => other.clone(),
    }
}

/// The command a call carries (`command` / `cmd` / `shell` / `script`), or
/// its arguments as JSON.
pub(crate) fn command_from_args(arguments: &Value) -> String {
    for key in ["command", "cmd", "shell", "script"] {
        if let Some(c) = arguments.get(key).and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty()) {
            return c.to_owned();
        }
    }
    if let Some(raw) = arguments.as_str() {
        if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
            return command_from_args(&parsed);
        }
        return raw.to_owned();
    }
    if arguments.is_null() || arguments.as_object().is_some_and(|m| m.is_empty()) {
        return String::new();
    }
    arguments.to_string()
}

/// A frame's field, or its `value`'s.
fn string_at(event: &Value, key: &str) -> Option<String> {
    event
        .get(key)
        .and_then(Value::as_str)
        .or_else(|| event.get("value").and_then(|v| v.get(key)).and_then(Value::as_str))
        .map(str::to_owned)
}
