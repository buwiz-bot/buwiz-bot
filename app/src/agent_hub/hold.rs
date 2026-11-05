//! A call the AI's press led to, held unsent until the person answers.
//!
//! A call that needs the person (a person-only tool whose plugin opted in,
//! or a write at Ask) stops here, in the bridge, before anything is signed.
//! The bridge keeps its tool and arguments. Those arguments are what goes
//! out, so there is nothing to copy back from the screen or the broker.
//!
//! - [`open`] mints the one-time id and is the only press open: one at a
//!   time. Any other call, the same or with other values, is refused and
//!   leaves the card the person is reading alone. Changed values can't reach
//!   the held call: what goes out is the arguments it was held with.
//! - [`Card`] is what the person reads, from the plugin's own read of the
//!   call. It stays in the app: it goes to no log and never to the model. A
//!   card is whole or there is none: what is too long for the sheet refuses
//!   the press, and nothing is ever cut to fit, because the hashes bind what
//!   the card holds and the yes has to cover what the call does.
//! - [`release`] is the only way to a press record, and it takes a
//!   [`PersonYes`], which only a real press on the card makes (`card.rs`).
//!   It retires the id first, so an Allow works once whatever follows. It
//!   compares what is about to go out with what the person saw itself, for
//!   every held call: [`authorize_press`] looks at an approval only for what
//!   needs a person, and a held draft doesn't. The `Caller` and `Approval` are
//!   built here, and whatever went through a card carries the person's yes.
//!
//! Nothing here is signed apart: an approval never leaves the process
//! except inside the call's own envelope, under the session key.

use std::sync::Mutex;

use buwiz_agent::{ActorId, Approval, Caller, ContentHash, Effect, OnceId, PressAction, PressedBy, Refused, SafeU64, authorize_press};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::press;
use crate::mini_v1::{RoxError, jcs, keys};

/// Who says yes, as the record names them: the one person at this Bot.
const PERSON: &str = "person.owner";
/// What a sheet shows, by the scroll view in `card.rs`. Past this a press is
/// refused: nobody reads more than this before pressing Allow.
const MAX_TITLE: usize = 120;
const MAX_LINE: usize = 400;
const MAX_LINES: usize = 80;

/// The person's own press of Allow. It can be made in two places: `card.rs`,
/// after it saw the press come from a person's hand, and tests. A broker
/// answer, or anything a message says, has no way to one.
#[derive(Debug)]
pub struct PersonYes(());

impl PersonYes {
    pub(super) fn by_hand() -> PersonYes {
        PersonYes(())
    }
}

/// How a held call ended.
#[derive(Debug)]
pub enum Answer {
    Allow(PersonYes),
    NotNow,
    /// The screen under the card changed, or another call came.
    Changed,
    /// The screen under the card closed.
    Closed,
    /// Nobody answered.
    TimedOut,
    /// The call couldn't be put in plain words.
    NoCard,
}

/// What the person reads: what will happen, with its real values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub app: String,
    pub title: String,
    pub lines: Vec<String>,
}

fn hash_of(bytes: &[u8]) -> Option<ContentHash> {
    let hex: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    ContentHash::parse(&format!("sha256:{hex}")).ok()
}

/// `sha256:` over `args`' JCS bytes: what an approval is tied to, and what
/// the plugin checks against the call it gets.
pub fn args_hash(args: &Value) -> Option<ContentHash> {
    hash_of(&jcs::signed_bytes(args).ok()?)
}

/// Whether `c` changes how a line reads without showing: control characters,
/// bidi overrides and isolates, zero-width and other format characters, and
/// the line separators, which would start a line nobody counted.
fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(c as u32,
            0xAD | 0x61C | 0x180E | 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x206F
            | 0xFE00..=0xFE0F | 0xFEFF | 0xFFF9..=0xFFFB | 0xE0000..=0xE0FFF)
}

impl Card {
    /// Its hash: what the person saw, word for word.
    pub fn hash(&self) -> Option<ContentHash> {
        hash_of(&jcs::signed_bytes(&json!({"app": self.app, "title": self.title, "lines": self.lines})).ok()?)
    }

    /// A card of `title` and `lines`, if it can be shown whole: no ticket,
    /// nothing empty, nothing past the sheet, and nothing that reads other
    /// than it is ([`hidden`]).
    fn whole(app: &str, title: String, lines: Vec<String>) -> Option<Card> {
        let fits = title.chars().count() <= MAX_TITLE && !lines.is_empty() && lines.len() <= MAX_LINES && lines.iter().all(|l| !l.is_empty() && l.chars().count() <= MAX_LINE);
        let shown = !title.is_empty() && fits && !lines.iter().chain([&title]).any(|l| l.contains("tkt_") || l.chars().any(hidden));
        shown.then(|| Card { app: app.to_owned(), title, lines })
    }

    /// The card a plugin's read answered with: `{title, lines}`, each line
    /// text. `None` for anything else, or a card that isn't [`Card::whole`].
    pub fn from_plugin(app: &str, v: &Value) -> Option<Card> {
        let title = v.get("title")?.as_str()?.trim().to_owned();
        let lines: Option<Vec<String>> = v.get("lines")?.as_array()?.iter().map(|l| l.as_str().map(|l| l.trim().to_owned())).collect();
        Card::whole(app, title, lines?)
    }

    /// The card of a write whose plugin has no read for it: its arguments
    /// as they are, all of them but `client_request_id`, which changes
    /// nothing saved. Only a write the AI may do anyway gets one of these:
    /// [`Effect::needs_person`] calls need the plugin's own words. A ticket
    /// has no words to show here, so a call holding one has no card.
    pub fn from_args(app: &str, args: &Value) -> Option<Card> {
        let mut lines = Vec::new();
        for (key, value) in args.as_object()?.iter().filter(|(k, _)| *k != "client_request_id") {
            let text = value.as_str().map_or_else(|| value.to_string(), str::to_owned);
            let label = key.replace('_', " ");
            let mut chars = label.chars();
            let label: String = chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default();
            lines.push(format!("{label}: {text}"));
        }
        Card::whole(app, format!("Save this in {app}?"), lines)
    }
}

/// The press open now.
struct OpenPress {
    once: OnceId,
    args_hash: ContentHash,
    /// What the card showed, once it did.
    shown: Option<ContentHash>,
}

static OPEN: Mutex<Option<OpenPress>> = Mutex::new(None);

fn lock() -> std::sync::MutexGuard<'static, Option<OpenPress>> {
    OPEN.lock().unwrap_or_else(|e| e.into_inner())
}

const BUSY: &str = "A card for the AI's last press is still waiting, so this one was refused and nothing was sent.";
const NO_HASH: &str = "These values can't be shown exactly, so the press was refused and nothing was sent.";

fn refused(said: &str) -> RoxError {
    press::note_refused(press::Held::Said(said.to_owned()));
    RoxError::new("permission_denied", said)
}

/// A fresh one-time id, like `press-…`.
fn mint() -> Option<OnceId> {
    let hex: String = keys::random_id().bytes().map(|b| format!("{b:02x}")).take(16).collect();
    OnceId::parse(&format!("press-{hex}")).ok()
}

/// Opens a press for a call with `args`: its one-time id.
pub fn open(args: &Value) -> Result<OnceId, RoxError> {
    let hash = args_hash(args).ok_or_else(|| refused(NO_HASH))?;
    let mut open = lock();
    if open.is_some() {
        return Err(refused(BUSY));
    }
    let once = mint().ok_or_else(|| refused(NO_HASH))?;
    *open = Some(OpenPress { once: once.clone(), args_hash: hash, shown: None });
    Ok(once)
}

/// The card went up saying `shown`.
pub fn showing(once: &OnceId, shown: ContentHash) {
    if let Some(open) = lock().as_mut().filter(|o| o.once == *once) {
        open.shown = Some(shown);
    }
}

/// `once` is over, however it ended; a second call finds nothing.
pub fn retire(once: &OnceId) {
    let mut open = lock();
    if open.as_ref().is_some_and(|o| o.once == *once) {
        *open = None;
    }
}

/// Lets `action` go, on the person's yes to `once`: the press record to sign
/// with it. `args` is what is about to go out and `shown` what the card says
/// now. The id is retired whatever comes of it.
pub fn release(yes: PersonYes, once: &OnceId, mut action: PressAction, args: &Value, shown: Option<ContentHash>, now_ms: i64) -> Result<Value, Refused> {
    let PersonYes(()) = yes;
    let open = lock().take_if(|o| o.once == *once).ok_or(Refused::NoOpenPress)?;
    let seen = open.shown.ok_or(Refused::NoOpenPress)?;
    if args_hash(args).as_ref() != Some(&open.args_hash) {
        return Err(Refused::ApprovalWrongArgs);
    }
    if shown.as_ref() != Some(&seen) {
        return Err(Refused::ApprovalWrongShown);
    }
    action.once = Some(open.once.clone());
    action.args_hash = Some(open.args_hash.clone());
    action.shown_hash = Some(seen.clone());
    let approved_by = ActorId::parse(PERSON).map_err(|_| Refused::ApprovalNotPerson)?;
    let at = u64::try_from(now_ms).ok().and_then(|ms| SafeU64::try_from(ms).ok()).ok_or(Refused::NoOpenPress)?;
    let approval = Approval {
        target: action.target.clone(),
        action: action.id.clone(),
        once: open.once.clone(),
        approved_by: approved_by.clone(),
        args_hash: open.args_hash.clone(),
        shown_hash: seen.clone(),
        at,
    };
    let Some(Caller::Agent { who, .. }) = press::caller() else { return Err(Refused::UnknownCaller) };
    let caller = Caller::Agent { who: who.clone(), approval: Some(approval) };
    authorize_press(Some(&action), Some(&caller))?;
    Ok(json!(PressedBy::AiApproved { who, approved_by, once: open.once, args_hash: open.args_hash, shown_hash: seen, at }))
}

/// Whether the effect may be shown from its bare arguments.
pub fn needs_plugin_words(effect: Effect) -> bool {
    effect.needs_person()
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::MutexGuard;

    use buwiz_agent::{AgentId, PressedBy, Target};

    use super::*;

    /// One press at a time, in the process: so one test at a time.
    pub(crate) fn serial() -> MutexGuard<'static, ()> {
        static ONE: Mutex<()> = Mutex::new(());
        let guard = ONE.lock().unwrap_or_else(|e| e.into_inner());
        *lock() = None;
        guard
    }

    fn post(effect: Effect) -> PressAction {
        PressAction { target: Target::parse("books").unwrap(), id: AgentId::parse("tool.books-app-post-entry").unwrap(), effect, once: None, args_hash: None, shown_hash: None }
    }

    fn entry() -> Value {
        json!({"entry": "ticket-1", "client_request_id": "post-1"})
    }

    fn card() -> Card {
        Card { app: "Books".into(), title: "Post this entry?".into(), lines: vec!["Rent, ₱1,350.00".into()] }
    }

    /// A press open on `args` with the card up.
    fn opened(args: &Value) -> OnceId {
        let once = open(args).unwrap();
        showing(&once, card().hash().unwrap());
        once
    }

    pub(crate) fn yes() -> PersonYes {
        PersonYes::by_hand()
    }

    fn released(once: &OnceId, args: &Value, shown: Card) -> Result<Value, Refused> {
        release(yes(), once, post(Effect::Write), args, shown.hash(), 1_793_745_245_000)
    }

    #[test]
    fn allow_gives_the_record_of_the_ai_and_the_person_and_when() {
        let _one = serial();
        let once = opened(&entry());
        let got = released(&once, &entry(), card()).unwrap();
        let p: PressedBy = serde_json::from_value(got.clone()).unwrap();
        assert!(matches!(&p, PressedBy::AiApproved { who, approved_by, once: o, .. } if who.as_str() == "agent.broker" && approved_by.as_str() == "person.owner" && *o == once));
        assert_eq!(got["at"], 1_793_745_245_000i64);
        assert_eq!(got["args_hash"], json!(args_hash(&entry()).unwrap().as_str()));
        assert_eq!(got["shown_hash"], json!(card().hash().unwrap().as_str()));
    }

    #[test]
    fn a_second_use_of_an_allow_finds_nothing() {
        let _one = serial();
        let once = opened(&entry());
        assert!(released(&once, &entry(), card()).is_ok());
        assert_eq!(released(&once, &entry(), card()), Err(Refused::NoOpenPress));
    }

    #[test]
    fn other_arguments_are_refused_and_the_id_is_spent() {
        let _one = serial();
        let once = opened(&entry());
        let other = json!({"entry": "ticket-2", "client_request_id": "post-1"});
        assert_eq!(released(&once, &other, card()), Err(Refused::ApprovalWrongArgs));
        assert_eq!(released(&once, &entry(), card()), Err(Refused::NoOpenPress), "changed values cancelled it");
    }

    #[test]
    fn a_card_that_says_something_else_now_is_refused() {
        let _one = serial();
        let once = opened(&entry());
        let changed = Card { lines: vec!["Rent, ₱9,350.00".into()], ..card() };
        assert_eq!(released(&once, &entry(), changed), Err(Refused::ApprovalWrongShown));
    }

    #[test]
    fn a_press_whose_card_never_showed_cant_be_released() {
        let _one = serial();
        let once = open(&entry()).unwrap();
        assert_eq!(released(&once, &entry(), card()), Err(Refused::NoOpenPress));
    }

    #[test]
    fn a_forged_id_opens_nothing_and_leaves_the_press_open() {
        let _one = serial();
        let once = opened(&entry());
        let forged = OnceId::parse("press-made-up-by-the-model").unwrap();
        assert_eq!(released(&forged, &entry(), card()), Err(Refused::NoOpenPress));
        assert!(released(&once, &entry(), card()).is_ok());
    }

    #[test]
    fn one_press_at_a_time_and_another_leaves_the_card_up() {
        let _one = serial();
        let once = opened(&entry());
        assert_eq!(open(&entry()).unwrap_err().message, BUSY);
        let other = json!({"entry": "ticket-2", "client_request_id": "post-2"});
        assert_eq!(open(&other).unwrap_err().message, BUSY);
        assert!(released(&once, &entry(), card()).is_ok(), "the card the person was reading still stands");
    }

    #[test]
    fn retiring_leaves_nothing_to_release() {
        let _one = serial();
        let once = opened(&entry());
        retire(&once);
        assert_eq!(released(&once, &entry(), card()), Err(Refused::NoOpenPress));
    }

    #[test]
    fn the_args_hash_is_the_jcs_sha256_every_side_computes() {
        let args = json!({"month": "2026-10", "client_request_id": "close-1"});
        // {"client_request_id":"close-1","month":"2026-10"}, as Books hashes it too
        assert_eq!(args_hash(&args).unwrap().as_str(), "sha256:cbf98bde1bdab782a73d98e6aa94663ac6786ef8d0e054ec61dd0cbb12a11830");
        assert_eq!(args_hash(&json!({"amount": 1.5})), None, "no exact bytes, no hash");
    }

    #[test]
    fn a_plugins_card_needs_a_title_and_lines_of_text_and_shows_no_ticket() {
        let good = json!({"title": "Post this entry?", "lines": ["Rent, ₱1,350.00"]});
        assert_eq!(Card::from_plugin("Books", &good).unwrap().lines, ["Rent, ₱1,350.00"]);
        for bad in [
            json!({"title": "x"}),
            json!({"lines": ["x"]}),
            json!({"title": "", "lines": ["x"]}),
            json!({"title": "x", "lines": []}),
            json!({"title": "x", "lines": ["tkt_0123"]}),
            json!({"title": "tkt_0123", "lines": ["x"]}),
            json!({"title": "x", "lines": ["x", 7]}),
            json!({"title": "x", "lines": ["x", ""]}),
        ] {
            assert_eq!(Card::from_plugin("Books", &bad), None, "{bad}");
        }
    }

    /// The hash binds what the card holds, so a card holds all of it or
    /// there is no card: the last line past each limit is what would be lost.
    #[test]
    fn a_card_too_long_to_show_is_no_card_and_nothing_is_cut() {
        let line = |n: usize| "x".repeat(n);
        let lines = |n: usize| (0..n).map(|i| format!("Entry {i}")).collect::<Vec<_>>();
        let card = |title: String, lines: Vec<String>| Card::from_plugin("Books", &json!({"title": title, "lines": lines}));
        let full = card("t".into(), lines(MAX_LINES)).unwrap();
        assert_eq!(full.lines.len(), MAX_LINES);
        assert_eq!(full.lines[MAX_LINES - 1], format!("Entry {}", MAX_LINES - 1), "every line, as sent");
        assert_eq!(card("t".into(), vec![line(MAX_LINE)]).unwrap().lines[0].len(), MAX_LINE);
        assert_eq!(card(line(MAX_TITLE), lines(1)).unwrap().title.len(), MAX_TITLE);
        assert_eq!(card("t".into(), lines(MAX_LINES + 1)), None, "too many lines");
        assert_eq!(card("t".into(), vec![line(MAX_LINE + 1)]), None, "too long a line");
        assert_eq!(card(line(MAX_TITLE + 1), lines(1)), None, "too long a title");
    }

    #[test]
    fn a_bare_card_shows_every_value_and_a_ticket_refuses_it() {
        let c = Card::from_args("eBIRForms", &json!({"client_request_id": "x", "tax_year": 2026, "due_amount": "1,350.00", "note": null})).unwrap();
        assert_eq!(c.title, "Save this in eBIRForms?");
        assert_eq!(c.lines, ["Due amount: 1,350.00", "Note: null", "Tax year: 2026"]);
        assert_eq!(Card::from_args("eBIRForms", &json!({"form": "tkt_0123", "tax_year": 2026})), None, "no dropping it and carrying on");
        assert_eq!(Card::from_args("eBIRForms", &json!({"forms": ["tkt_0123"]})), None, "nested too");
        assert_eq!(Card::from_args("eBIRForms", &json!({"client_request_id": "x"})), None, "nothing to show");
        assert_eq!(Card::from_args("eBIRForms", &json!({"note": "x".repeat(MAX_LINE)})), None, "too long");
    }

    #[test]
    fn a_person_only_call_never_gets_a_bare_card() {
        assert!(needs_plugin_words(Effect::Write) && needs_plugin_words(Effect::File));
        assert!(!needs_plugin_words(Effect::Draft));
    }

    #[test]
    fn a_draft_allowed_on_the_card_carries_the_persons_yes() {
        let _one = serial();
        let once = opened(&entry());
        let got = release(yes(), &once, post(Effect::Draft), &entry(), card().hash(), 1).unwrap();
        assert_eq!(got["by"], "ai_approved");
        assert_eq!(got["approved_by"], "person.owner");
        assert_eq!(got["once"], json!(once.as_str()));
        assert_eq!(got["args_hash"], json!(args_hash(&entry()).unwrap().as_str()));
        assert_eq!(got["shown_hash"], json!(card().hash().unwrap().as_str()));
        assert!(serde_json::from_value::<PressedBy>(got).is_ok(), "the broker takes it");
    }

    /// `authorize_press` doesn't look at an approval on a draft, so this is
    /// `release`'s own to refuse.
    #[test]
    fn a_held_draft_with_a_changed_card_or_other_arguments_is_refused() {
        let _one = serial();
        let once = opened(&entry());
        let changed = Card { lines: vec!["Rent, ₱9,350.00".into()], ..card() };
        assert_eq!(release(yes(), &once, post(Effect::Draft), &entry(), changed.hash(), 1), Err(Refused::ApprovalWrongShown));
        assert_eq!(release(yes(), &once, post(Effect::Draft), &entry(), card().hash(), 1), Err(Refused::NoOpenPress), "spent");
        let once = opened(&entry());
        let other = json!({"entry": "ticket-2", "client_request_id": "post-1"});
        assert_eq!(release(yes(), &once, post(Effect::Draft), &other, card().hash(), 1), Err(Refused::ApprovalWrongArgs));
        let once = opened(&entry());
        assert_eq!(release(yes(), &once, post(Effect::Draft), &entry(), None, 1), Err(Refused::ApprovalWrongShown), "no card now");
    }

    #[test]
    fn a_filing_is_refused_with_a_card_and_a_yes() {
        let _one = serial();
        let once = opened(&entry());
        assert_eq!(release(yes(), &once, post(Effect::File), &entry(), card().hash(), 1), Err(Refused::FilingNotForAgents));
    }

    #[test]
    fn a_line_that_reads_other_than_it_is_is_no_card() {
        let good = |t: &str| Card::from_plugin("Books", &json!({"title": "t", "lines": [t]})).is_some();
        assert!(good("Rent, ₱1,350.00 — café, 東京"));
        for bad in ["\u{202E}00.053,1₱", "a\u{2066}b", "a\u{200B}b", "a\u{200D}b", "a\u{FEFF}b", "a\u{AD}b", "a\u{2028}b", "a\nb", "a\tb", "a\u{1}b", "a\u{E0041}b", "a\u{FE0F}b"] {
            assert!(!good(bad), "{bad:?}");
        }
        assert_eq!(Card::from_plugin("Books", &json!({"title": "a\u{202E}b", "lines": ["x"]})), None, "the title too");
        assert_eq!(Card::from_args("eBIRForms", &json!({"note": "a\u{202E}b"})), None);
    }
}
