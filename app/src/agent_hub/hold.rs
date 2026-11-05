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
//!   call. It stays in the app: it goes to no log and never to the model.
//! - [`release`] is the only way to a press record, and it takes a
//!   [`PersonYes`], which only a real press on the card makes (`card.rs`).
//!   It retires the id first, so an Allow works once whatever follows. The
//!   `Caller` and `Approval` are built here, and [`authorize_press`] decides
//!   on the hashes of what is about to go out against what the person saw.
//!
//! Nothing here is signed apart: an approval never leaves the process
//! except inside the call's own envelope, under the session key.

use std::sync::Mutex;

use buwiz_agent::{ActorId, Approval, Caller, ContentHash, Effect, OnceId, PressAction, Refused, SafeU64, authorize_press};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::press;
use crate::mini_v1::{RoxError, jcs, keys};

/// Who says yes, as the record names them: the one person at this Bot.
const PERSON: &str = "person.owner";
const MAX_TITLE: usize = 120;
const MAX_LINE: usize = 200;
const MAX_LINES: usize = 12;

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

impl Card {
    /// Its hash: what the person saw, word for word.
    pub fn hash(&self) -> Option<ContentHash> {
        hash_of(&jcs::signed_bytes(&json!({"app": self.app, "title": self.title, "lines": self.lines})).ok()?)
    }

    /// The card a plugin's read answered with: `{title, lines}`. `None`
    /// without both, or with a ticket showing.
    pub fn from_plugin(app: &str, v: &Value) -> Option<Card> {
        let clip = |s: &str, n: usize| s.trim().chars().take(n).collect::<String>();
        let title = clip(v.get("title")?.as_str()?, MAX_TITLE);
        let lines: Vec<String> = v.get("lines")?.as_array()?.iter().filter_map(Value::as_str).map(|l| clip(l, MAX_LINE)).filter(|l| !l.is_empty()).take(MAX_LINES).collect();
        let shown = !title.is_empty() && !lines.is_empty() && !lines.iter().chain([&title]).any(|l| l.contains("tkt_"));
        shown.then(|| Card { app: app.to_owned(), title, lines })
    }

    /// The card of a write whose plugin has no read for it: its arguments
    /// as they are. Only a write the AI may do anyway gets one of these:
    /// [`Effect::needs_person`] calls need the plugin's own words.
    pub fn from_args(app: &str, args: &Value) -> Card {
        let mut lines = Vec::new();
        for (key, value) in args.as_object().into_iter().flatten() {
            let text = match value {
                Value::String(s) => s.clone(),
                Value::Null => continue,
                other => other.to_string(),
            };
            if key != "client_request_id" && !text.contains("tkt_") && !text.is_empty() {
                let label = key.replace('_', " ");
                let mut chars = label.chars();
                let label: String = chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default();
                lines.push(format!("{label}: {text}").chars().take(MAX_LINE).collect());
            }
        }
        lines.truncate(MAX_LINES);
        Card { app: app.to_owned(), title: format!("Save this in {app}?"), lines }
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
    action.once = Some(open.once.clone());
    action.args_hash = args_hash(args);
    action.shown_hash = shown;
    let approval = Approval {
        target: action.target.clone(),
        action: action.id.clone(),
        once: open.once,
        approved_by: ActorId::parse(PERSON).map_err(|_| Refused::ApprovalNotPerson)?,
        args_hash: open.args_hash,
        shown_hash: open.shown.ok_or(Refused::NoOpenPress)?,
        at: u64::try_from(now_ms).ok().and_then(|ms| SafeU64::try_from(ms).ok()).ok_or(Refused::NoOpenPress)?,
    };
    let Some(Caller::Agent { who, .. }) = press::caller() else { return Err(Refused::UnknownCaller) };
    let caller = Caller::Agent { who, approval: Some(approval) };
    let allowed = authorize_press(Some(&action), Some(&caller))?;
    Ok(json!(allowed.pressed))
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
    fn a_plugins_card_needs_a_title_and_a_line_and_shows_no_ticket() {
        let good = json!({"title": "Post this entry?", "lines": ["Rent, ₱1,350.00", "", 7]});
        assert_eq!(Card::from_plugin("Books", &good).unwrap().lines, ["Rent, ₱1,350.00"]);
        for bad in [json!({"title": "x"}), json!({"lines": ["x"]}), json!({"title": "", "lines": ["x"]}), json!({"title": "x", "lines": []}), json!({"title": "x", "lines": ["tkt_0123"]})] {
            assert_eq!(Card::from_plugin("Books", &bad), None, "{bad}");
        }
    }

    #[test]
    fn a_bare_card_shows_the_values_and_leaves_out_ids_and_tickets() {
        let c = Card::from_args("eBIRForms", &json!({"client_request_id": "x", "form": "tkt_0123", "tax_year": 2026, "due_amount": "1,350.00", "note": null}));
        assert_eq!(c.title, "Save this in eBIRForms?");
        assert_eq!(c.lines, ["Due amount: 1,350.00", "Tax year: 2026"]);
    }

    #[test]
    fn a_person_only_call_never_gets_a_bare_card() {
        assert!(needs_plugin_words(Effect::Write) && needs_plugin_words(Effect::File));
        assert!(!needs_plugin_words(Effect::Draft));
    }

    #[test]
    fn a_draft_allowed_on_the_card_goes_out_as_the_ais_press() {
        let _one = serial();
        let once = opened(&entry());
        let got = release(yes(), &once, post(Effect::Draft), &entry(), card().hash(), 1).unwrap();
        assert_eq!(got, json!({"by": "ai", "who": "agent.broker"}));
    }
}
