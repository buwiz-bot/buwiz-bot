//! `request_user_form`: a login card in the chat, whose values the broker types into the Bot's own
//! browser, out of the model's view. Ported from OpenGrok's `request_user_form`
//! (opengrok-forms `user_form.rs`, `front_page.rs`; opengrok-server `agui/user_form.rs`).
//!
//! - The model names the fields, the site and how the page is laid out. Any value it puts on a
//!   field is dropped ([`sanitize`]): a card never carries the model's words as a prefill.
//! - The card is a `user_form` notice in the chat. The run waits on it, for [`HOLD`] at most, and
//!   is the one that types: the person's answer comes to it over `POST
//!   /v1/host/computer/login/submit` and goes nowhere else. Nothing is stored: the values live in
//!   that request and in the run's fill, and are dropped once it's done.
//! - The fill goes through the box's `/v1/browser/*`, by the refs a snap gives, never by the
//!   model's `computer_*` tools and never with a picture ([`fill`]). Every call carries the card's
//!   site as `allow_domains`. The page in front must be on that site, by the box's extension
//!   (`GET /v1/chrome/active-tab`) before anything is typed, and by `box-exec`'s own snap before
//!   each value. A password goes only into a field the box calls sensitive, and a plain value only
//!   into one it doesn't. Return is pressed only by OpenGrok's rule ([`should_press_return`]), and a
//!   press the box asks a person to confirm is never confirmed here.
//! - The model reads that the form was filled, never that anyone signed in, and only the values of
//!   fields that aren't secret ([`result_text`]). It looks for itself afterwards.
//! - A saved login (the app's vault, behind Touch ID) is typed only on a computer this Bot doesn't
//!   share: OpenGrok's 403 `shared-computer`.
//! - Who decides what about a saved login. The vault, and which Bots each login is shared with,
//!   live in the app on the Mac; this broker never sees them, so the app alone decides "shared
//!   with this Bot" (and share-all), and refuses an unshared login before Touch ID and before
//!   anything is sent. This broker alone decides "this Bot's own computer": it refuses a saved
//!   login on a computer several Bots share, whatever the app sends, share-all included. It isn't
//!   told the shares: a claim from the app would come from the same host-token holder that
//!   already decides them, so it would check nothing. OpenGrok's `not-shared` 403 has no
//!   counterpart here for that reason.
//! - "Open the screen" is the hand-over `computer_ask_person` makes, without a second card: the
//!   person asked for it on this one.
//! - Logs name a field by its id and label, and say how long its value was. Never a value.
//! - Why no Ask card comes first (it's in `computer::READS`, under the `computer` switch): the
//!   card is the person's own yes. It names the site it types into and says when Return is pressed,
//!   and nothing reaches the box, not even a look at the page, until a person presses Log in (or
//!   Open the screen, which hands the screen to them). A switch at Off still takes the tool away.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use buwiz_computer::{BrowserOp, BrowserReply, Secret};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use super::computer::{self as tools, TAKEOVER_NOTE};
use super::{Out, Run};
use crate::hub::computer::{Computers, Live};
use crate::hub::ctx::HubCtx;
use crate::hub::policy::ask as asking;
use crate::js;

pub const NAME: &str = "request_user_form";
/// The kind of the card's notice.
pub const KIND: &str = "user_form";
/// How long a card waits for its answer (OpenGrok's `FORM_HOLD_TIMEOUT`).
pub const HOLD: Duration = Duration::from_secs(600);
/// Another hold than [`HOLD`] (tests), in `ctx.ext`.
pub struct LoginWait(pub Duration);

const MAX_FIELDS: usize = 6;
const MAX_ID: usize = 64;
const MAX_LABEL: usize = 80;
const MAX_TITLE: usize = 120;
const MAX_INSTRUCTION: usize = 300;
const MAX_VALUE: usize = 4096;
/// How long a submit waits for the run to have typed it.
const ANSWER_PATIENCE: Duration = Duration::from_secs(120);
/// Another patience than [`ANSWER_PATIENCE`] (tests), in `ctx.ext`.
pub struct AnswerWait(pub Duration);
/// How a hand-over from the card is limited, as `computer_ask_person`'s default.
const HANDOVER_S: u64 = 300;

pub const SECRET_KINDS: [&str; 2] = ["password", "otp"];

/// Words that make a field a secret, whatever the model called it: the reference's and the box's
/// (ultra-instinct's `SECRET_LABEL_WORDS`), and a few more.
const SECRET_WORDS: [&str; 15] = ["password", "passwd", "passcode", "passphrase", "pin", "cvv", "cvc", "ssn", "card", "security", "secret", "otp", "totp", "2fa", "mfa"];
/// Of those, the ones that make it a one-time code, which a page may ask for in a plain box.
const CODE_WORDS: [&str; 5] = ["otp", "totp", "2fa", "mfa", "one-time"];
/// Phrases that make it a one-time code.
const CODE_PHRASES: [&str; 6] = ["one time", "verification code", "authentication code", "auth code", "sms code", "security code"];

/// What a field's words say it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hint {
    Plain,
    /// a password or the like: it goes only into a box the page calls sensitive
    Secret,
    /// a one-time code
    Code,
}

/// What `text` (a label, an id, a type) names: split on anything but letters, digits and `-`, as the
/// box splits a label, and on `_` and `.` too.
pub fn hint_of(text: &str) -> Hint {
    let lower = text.to_lowercase();
    let spaced: String = lower.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { ' ' }).collect();
    let words: Vec<&str> = spaced.split_whitespace().collect();
    let phrase = words.join(" ");
    if words.iter().any(|w| CODE_WORDS.contains(w)) || CODE_PHRASES.iter().any(|p| phrase.contains(p)) {
        Hint::Code
    } else if words.iter().any(|w| SECRET_WORDS.contains(w) || w.ends_with("password")) {
        Hint::Secret
    } else {
        Hint::Plain
    }
}
const CHALLENGES: [&str; 4] = ["password", "otp", "captcha", "outside_sandbox"];

/// The text a sanitised form keeps, field by field: never a value.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub required: bool,
    pub secret: bool,
}

impl Field {
    /// Masked on the card, and never shown to the model: the secret flag, or a password or code.
    pub fn is_secret(&self) -> bool {
        self.secret || SECRET_KINDS.contains(&self.kind.as_str())
    }

    /// Goes only into a field the box calls sensitive (OpenGrok's `wants_a_password_box`): a
    /// password, or anything marked secret that isn't a one-time code, which a page may well ask
    /// for in a plain box.
    pub fn wants_a_password_box(&self) -> bool {
        (self.kind == "password" || self.secret) && self.kind != "otp"
    }
}

/// A card as the model asked for it, less anything it may not say.
#[derive(Clone, Debug, PartialEq)]
pub struct Form {
    pub title: String,
    pub instruction: String,
    pub fields: Vec<Field>,
    /// the site the page is on, a bare lowercase host, `www.` dropped
    pub site: String,
    pub challenge_kind: Option<String>,
    pub same_page: bool,
    pub submit: bool,
}

impl Form {
    pub fn to_json(&self) -> Value {
        let fields: Vec<Value> = self.fields.iter().map(|f| json!({"id": f.id, "label": f.label, "type": f.kind, "required": f.required, "secret": f.is_secret()})).collect();
        let mut v = json!({"title": self.title, "instruction": self.instruction, "fields": fields, "domain": self.site, "samePage": self.same_page, "submit": self.submit});
        if let Some(kind) = &self.challenge_kind {
            v["challengeKind"] = json!(kind);
        }
        v
    }

    /// Several fields on a card not marked one-page: only the first is typed (OpenGrok's
    /// `types_only_first_field`; refs stand in for its positions, so `samePage` is the mark).
    pub fn types_only_first_field(&self) -> bool {
        self.fields.len() > 1 && !self.same_page
    }

    fn has_secret(&self) -> bool {
        self.fields.iter().any(Field::is_secret)
    }
}

/// The schema the model gets.
pub fn def() -> Value {
    json!({
        "name": NAME,
        "description": "Ask the person to sign in on a page in your screen's browser: a card in the chat where they type the login, or pick one saved on their Mac and confirm with Touch ID. The values are typed into the page out of your view, by the page's own fields, and you never see a password or code. Use this, not computer_type, for every password, one-time code or other secret, and never ask for one in chat. Open the sign-in page first with computer_open_url, look at it, then call this with the page's site as domain and one field per box you see, labelled as the page labels it. When the page shows the email and password together, raise ONE card with both and samePage true, and submit true to press Return after; a page that asks for the email alone gets an email-only card, and the password page after it a password-only card. After it settles, take a screenshot and confirm what the page shows: filling is not signing in. A captcha, an SMS or app prompt, or anything odd on the page isn't a form: use computer_ask_person. If they dismiss it, continue without the login and do not raise the same card again.",
        "input_schema": {
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Short title shown on the card, such as \"Sign in to Shop\"."},
                "instruction": {"type": "string", "description": "What the person should do, in one line."},
                "domain": {"type": "string", "description": "The site the sign-in page is on, as its address shows it: shop.example. Nothing is typed on another site."},
                "liveHost": {"type": "string", "description": "The host on the screen now, when it's a sign-in host under domain (accounts.shop.example). It never changes the site: the fill holds to domain and the hosts under it."},
                "challengeKind": {"type": "string", "enum": CHALLENGES, "description": "password or otp. captcha and outside_sandbox aren't forms: use computer_ask_person."},
                "samePage": {"type": "boolean", "description": "The fields share one page (an email and a password together). Set it on every one-page card: without it only the first field is typed."},
                "submit": {"type": "boolean", "description": "Press Return in the last field after the fill: the page's own Sign in. Set it on a one-page login card. A single field presses it on its own."},
                "fields": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string"},
                            "label": {"type": "string", "description": "The field's label as the page shows it."},
                            "type": {"type": "string", "description": "email, text, tel, password or otp"},
                            "required": {"type": "boolean"},
                            "secret": {"type": "boolean", "description": "Mask it; password and otp are secret anyway."}
                        },
                        "required": ["id", "label"]
                    }
                }
            },
            "required": ["title", "fields", "domain"]
        }
    })
}

fn text(input: &Value, key: &str, max: usize) -> String {
    let t = input.get(key).and_then(Value::as_str).unwrap_or_default().trim();
    js::slice_utf16(t, max).to_owned()
}

/// `domain` (or `liveHost`) as a bare host: a scheme, a path, a port and `www.` go. `None` for
/// anything that isn't a host name or an address.
pub fn site_of(raw: &str) -> Option<String> {
    let raw = raw.trim();
    // a backslash ends the authority, as a browser reads it: `evil.test\@shop.example` is evil.test
    let raw = raw.replace('\\', "/");
    let raw = raw.as_str();
    let rest = raw.split_once("://").map_or(raw, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or("");
    let host = authority.split(':').next().unwrap_or("").trim_end_matches('.').to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_owned();
    let ok = !host.is_empty() && host.len() <= 253 && host.split('.').all(|p| !p.is_empty() && p.len() <= 63 && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    ok.then_some(host)
}

/// The host of a page's `http(s)` address, as [`site_of`] gives it. Any other scheme has none.
pub fn page_host(url: &str) -> Option<String> {
    let lower = url.trim().to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return None;
    }
    site_of(url)
}

/// Whether a page at `url` is on `site`: the host itself or one under it.
pub fn on_site(url: &str, site: &str) -> bool {
    page_host(url).is_some_and(|host| host == site || host.ends_with(&format!(".{site}")))
}

/// The card the model's call asks for, with every value it put on a field dropped (OpenGrok's
/// `sanitize_arguments` and `validate_field_ids`): only id, label, type, required and secret are
/// read of a field. `Err` is what the model is told.
pub fn sanitize(input: &Value) -> Result<Form, String> {
    let title = text(input, "title", MAX_TITLE);
    if title.is_empty() {
        return Err(format!("{NAME} needs a title"));
    }
    let challenge_kind = input.get("challengeKind").and_then(Value::as_str).map(|k| k.trim().to_ascii_lowercase()).filter(|k| !k.is_empty());
    if let Some(kind) = &challenge_kind {
        if kind == "captcha" || kind == "outside_sandbox" {
            return Err("A captcha or a page outside the browser isn't a form the person can fill here. Use computer_ask_person so they finish it on your screen.".to_owned());
        }
        if !CHALLENGES.contains(&kind.as_str()) {
            return Err("challengeKind is password or otp".to_owned());
        }
    }
    // `domain` is the site, the one the card shows and the fill holds to. `liveHost` says nothing
    // the page check doesn't: a host under the site already passes it, and any other is ignored, so
    // it can never move the card to another site
    let Some(site) = input.get("domain").and_then(Value::as_str).and_then(site_of) else { return Err(format!("{NAME} needs the site the page is on, as domain (shop.example): nothing is typed without one")) };
    let raw = input.get("fields").and_then(Value::as_array).cloned().unwrap_or_default();
    if raw.is_empty() {
        return Err(format!("{NAME} needs at least one field"));
    }
    if raw.len() > MAX_FIELDS {
        return Err(format!("A sign-in card takes at most {MAX_FIELDS} fields"));
    }
    let mut fields: Vec<Field> = Vec::new();
    for f in &raw {
        let id = text(f, "id", MAX_ID);
        let label = text(f, "label", MAX_LABEL);
        if id.is_empty() || label.is_empty() {
            return Err("Each field needs an id and a label".to_owned());
        }
        if !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')) {
            return Err("A field's id is letters, digits, _, - and . only".to_owned());
        }
        if fields.iter().any(|g| g.id == id) {
            return Err(format!("Field IDs must be unique. Give each field a different id and call {NAME} again."));
        }
        let raw = f.get("type").and_then(Value::as_str).map(|k| k.trim().to_ascii_lowercase()).filter(|k| !k.is_empty()).unwrap_or_else(|| "text".to_owned());
        let mut kind = if matches!(raw.as_str(), "email" | "text" | "tel" | "password" | "otp" | "number") { raw.clone() } else { "text".to_owned() };
        let mut secret = f.get("secret").and_then(Value::as_bool).unwrap_or(false);
        // secrecy isn't the model's to turn off: a field whose label, id or type names a secret is one
        let hints = [hint_of(&label), hint_of(&id), hint_of(&raw)];
        if kind != "password" && kind != "otp" {
            if hints.contains(&Hint::Secret) {
                secret = true;
            } else if hints.contains(&Hint::Code) {
                kind = "otp".to_owned();
            }
        }
        fields.push(Field { id, label, kind, required: f.get("required").and_then(Value::as_bool).unwrap_or(false), secret });
    }
    Ok(Form {
        title,
        instruction: text(input, "instruction", MAX_INSTRUCTION),
        fields,
        site,
        challenge_kind,
        same_page: input.get("samePage").and_then(Value::as_bool).unwrap_or(false),
        submit: input.get("submit").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// Return is unsafe on a stepped form; safe when asked for (`submit`), on a single field, on a
/// one-page card typed in full with a secret among it, or on an otp or password challenge that
/// typed one field (OpenGrok's `should_press_return`, with refs as its positions).
pub fn should_press_return(form: &Form, typed: usize) -> bool {
    if typed == 0 {
        return false;
    }
    if form.submit {
        return true;
    }
    if form.types_only_first_field() {
        return false;
    }
    if form.fields.len() == 1 {
        return true;
    }
    if form.same_page && typed == form.fields.len() && form.has_secret() {
        return true;
    }
    typed == 1 && matches!(form.challenge_kind.as_deref(), Some("otp" | "password"))
}

/// What becomes of Return, as far as the card can say before anything is typed: `always` (by
/// [`should_press_return`] whichever fields are filled, once any is), `never`, or `maybe` (it
/// depends on which are). The card's words before the press come from this, not from a copy of
/// the rule.
pub fn return_press(form: &Form) -> &'static str {
    let most = if form.types_only_first_field() { 1 } else { form.fields.len() };
    let presses: Vec<bool> = (1..=most).map(|typed| should_press_return(form, typed)).collect();
    match (presses.iter().all(|p| *p), presses.iter().any(|p| *p)) {
        _ if presses.is_empty() => "never",
        (true, _) => "always",
        (false, false) => "never",
        (false, true) => "maybe",
    }
}

// ---- the fill ----

/// What became of Return.
#[derive(Clone, Debug, PartialEq)]
pub enum Pressed {
    No,
    Yes,
    /// the box asks a person to confirm pressing this label: nobody did
    NeedsConfirm(String),
    Failed(String),
    /// the Enter went out and its answer didn't say how it went: it may have been pressed
    Uncertain(String),
}

/// One field's outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub id: String,
    pub filled: bool,
    /// it went into an element the box calls sensitive: whatever the card said, its value is a
    /// secret, never shown to the model
    pub sensitive: bool,
}

/// What a fill came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Fill {
    /// nothing was typed, and the card stays open for another try (OpenGrok's 403 `wrong-page`)
    Refused { code: &'static str, message: String },
    /// it stopped: what was typed before is in `outcomes`. `uncertain`: the last type went out and
    /// its answer didn't say how it went (a timeout, a broken answer, a press on another element),
    /// so a value may be in the page; `maybe_secret`: that value was a secret's
    Failed { outcomes: Vec<Outcome>, reason: String, uncertain: bool, maybe_secret: bool },
    Filled { outcomes: Vec<Outcome>, pressed: Pressed },
    /// every field the fill would type was empty: nothing was typed, and nothing reached the box
    Empty,
}

/// The sentence for an image without the browser layer on a `BOX_CHROME=0` computer.
pub const TOO_OLD: &str = "This computer's image is too old to type a login into its browser. Update the computer image and make this computer again from its Computer tab, or use Open the screen to sign in yourself.";
const LAYER_OFF: &str = "The browser layer is off on this computer, so nothing was typed. Use Open the screen to sign in yourself.";

/// Whether the box's image types into a browser by its controls: its `capabilities.browser`, which
/// an image from before `BOX_CHROME=0` allowed it hasn't got.
pub async fn browser_layer(live: &Live) -> Result<(), String> {
    let caps = live.provider.capabilities(&live.machine).await.map_err(|p| p.sentence())?;
    match caps.pointer("/browser/enabled").and_then(Value::as_bool) {
        Some(true) => Ok(()),
        Some(false) => Err(LAYER_OFF.to_owned()),
        None => Err(TOO_OLD.to_owned()),
    }
}

/// A box refusal as a sentence for the card and the model: the box's own words aren't passed on.
fn refusal_sentence(reply: &BrowserReply, site: &str) -> String {
    match reply.code().unwrap_or("") {
        "stale_ref" => "The page changed while it was being filled, so filling stopped.".to_owned(),
        "pressed_other" => "The typing landed on another part of the page than the field it was meant for, so filling stopped.".to_owned(),
        "domain_not_allowed" => format!("The page isn't on {site} any more, so nothing more was typed."),
        "browser_unavailable" => "No browser is open on this Bot's screen, so nothing was typed. Open the page first.".to_owned(),
        "browser_disabled" => LAYER_OFF.to_owned(),
        "browser_timeout" => "The browser didn't answer in time.".to_owned(),
        "busy" => "The browser was busy, so nothing more was typed. Try again.".to_owned(),
        "action_refused" => "The page wouldn't take the typing there: the field is covered, out of view or disabled.".to_owned(),
        "dialog_open" => "A dialog is open on the page, so nothing more was typed.".to_owned(),
        "target_gone" => "The page's tab went away, so nothing more was typed.".to_owned(),
        "confirm_required" => "The page asks a person to confirm that, so nothing was pressed.".to_owned(),
        code if !code.is_empty() => format!("The browser said no ({}), so filling stopped.", js::slice_utf16(code, 40)),
        _ => format!("The browser answered {}, so filling stopped.", reply.status),
    }
}

/// Whether a refused type may still have landed: the box ran out of time on it (`browser_timeout`,
/// which it says of a press already sent), or pressed another element (`pressed_other`). `busy`
/// wasn't run, `stale_ref` pressed nothing, and `target_gone` had no tab to type into.
fn uncertain(reply: &BrowserReply) -> bool {
    matches!(reply.code(), Some("browser_timeout" | "pressed_other")) || reply.status == 504
}

/// What the model and the card are told of a type that may have landed. The box's `type` sets a
/// field's whole value (ultra-instinct's DOM setter), so a new card's fill of that field replaces
/// it; a press on another element may have left a value there.
pub const MAYBE_TYPED: &str = "A value may have been typed into the page before it stopped: take a screenshot before anything else. A new card's fill sets each field's whole value rather than adding to it, but a press that landed on another element may have left a value there.";

/// The card's note when every field was empty.
pub const EMPTY_NOTE: &str = "Nothing was typed: every field was empty.";
const EMPTY_NEXT: &str = "Continue without those credentials, or ask the person in chat whether they meant to; do not raise the same card again.";

/// What the model and the card are told of an Enter that may have gone: looking first keeps a
/// login from going twice.
pub const MAYBE_PRESSED: &str = "Return may have been pressed: the browser didn't say how it went. Take a screenshot and look before pressing anything, so the login isn't sent twice.";

/// The site list for every fill call: the site and the hosts under it.
fn allow(site: &str) -> Value {
    json!([site, format!("*.{site}")])
}

/// Writes a line of the fill's log (and keeps it, in tests, for the scans that look for a value).
fn log(line: String) {
    #[cfg(test)]
    logged().push(line.clone());
    eprintln!("{line}");
}

/// What [`log`] wrote, in tests.
#[cfg(test)]
pub(crate) fn logged() -> MutexGuard<'static, Vec<String>> {
    static L: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    L.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// A line of the fill's log: a field by its id and label, and how long its value is. It takes no
/// value, so it can't show one.
pub fn field_log_line(field: &Field, secret: bool, len: usize, what: &str) -> String {
    format!("buwiz-broker: login: field {} ({:?}) secret={} len={len}: {what}", field.id, js::slice_utf16(&field.label, MAX_LABEL), secret || field.is_secret())
}

fn norm(s: &str) -> String {
    let lower: String = s.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { ' ' }).collect();
    lower.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The ref of the page's field for `field`, of those it may go into: a sensitive one for a
/// password, a plain one for a plain value, either for a code. By its label, whole, then in part,
/// then a label naming its kind (an email box for an email); a guess only for a password, into the
/// page's only sensitive field. `Err` names what's wrong. `taken` are the labels of fields already
/// typed, which no other field goes into.
fn pick(elements: &[Value], field: &Field, taken: &[String]) -> Result<String, String> {
    let typeable = |e: &&Value| e["actions"].as_array().is_some_and(|a| a.iter().any(|x| x == "type"));
    let sensitive = |e: &Value| e["sensitive"].as_bool().unwrap_or(false);
    let fits = |e: &&Value| {
        if field.wants_a_password_box() {
            sensitive(e)
        } else if field.is_secret() {
            true
        } else {
            !sensitive(e)
        }
    };
    let label_of = |e: &Value| norm(e["label"].as_str().unwrap_or_default());
    let open: Vec<&Value> = elements.iter().filter(typeable).filter(fits).filter(|e| !taken.contains(&label_of(e))).collect();
    let want = norm(&field.label);
    let one = |hits: Vec<&&Value>| -> Option<String> { if let [e] = hits.as_slice() { e["ref"].as_str().map(str::to_owned) } else { None } };
    if !want.is_empty() {
        if let Some(r) = one(open.iter().filter(|e| label_of(e) == want).collect()) {
            return Ok(r);
        }
        if let Some(r) = one(open.iter().filter(|e| { let l = label_of(e); !l.is_empty() && (l.contains(&want) || want.contains(&l)) }).collect()) {
            return Ok(r);
        }
    }
    // the page's label names what the field's kind is: an email box for an email
    let kind_words: &[&str] = match field.kind.as_str() {
        "email" => &["email", "e-mail", "e mail", "username", "user name", "login", "account"],
        "tel" => &["phone", "mobile", "tel", "telephone"],
        "otp" => &["code", "otp", "verification", "one-time", "one time", "passcode"],
        _ => &[],
    };
    if !kind_words.is_empty() {
        if let Some(r) = one(open.iter().filter(|e| { let l = label_of(e); kind_words.iter().any(|w| l.contains(w)) }).collect()) {
            return Ok(r);
        }
    }
    // a guess only for a password, into the page's only sensitive field: a plain value never goes
    // into whatever box is left
    if field.wants_a_password_box() && elements.iter().filter(typeable).filter(|e| sensitive(e)).count() == 1 {
        if let Some(r) = one(open.iter().collect()) {
            return Ok(r);
        }
    }
    if open.is_empty() && field.wants_a_password_box() {
        return Err(format!("The page has no password field for \"{}\", so a password would go into a box that isn't one, and nothing more was typed.", field.label));
    }
    if open.is_empty() {
        return Err(format!("The page has no field \"{}\" could go into, so nothing more was typed.", field.label));
    }
    Err(format!("Couldn't tell which field on the page is \"{}\", so nothing more was typed. Raise the card with the page's own label for it.", field.label))
}

/// Why a snap gave nothing to type into.
enum SnapErr {
    /// the page isn't on the card's site: its host
    OffSite(String),
    Other(String),
}

impl SnapErr {
    fn sentence(self, site: &str) -> String {
        match self {
            SnapErr::OffSite(host) => format!("The page in front is {host}, not {site}, so nothing more was typed."),
            SnapErr::Other(why) => why,
        }
    }
}

/// A snap of the page, on the card's site by `box-exec`'s own look: its number and its controls.
async fn snap(live: &Live, site: &str) -> Result<(u64, Vec<Value>), SnapErr> {
    let reply = live.provider.browser(&live.machine, &live.screen, BrowserOp::Snap, &json!({"allow_domains": allow(site), "max_nodes": 400})).await.map_err(|p| SnapErr::Other(p.sentence()))?;
    if !reply.ok() {
        return Err(SnapErr::Other(refusal_sentence(&reply, site)));
    }
    let url = reply.body["url"].as_str().unwrap_or_default();
    if !on_site(url, site) {
        return Err(SnapErr::OffSite(page_host(url).unwrap_or_else(|| "not a web page".to_owned())));
    }
    let n = reply.body["snapshot"].as_u64().ok_or_else(|| SnapErr::Other("The browser's look at the page had no number, so nothing more was typed.".to_owned()))?;
    Ok((n, reply.body["elements"].as_array().cloned().unwrap_or_default()))
}

/// The page in front, by the box's extension: on the card's site, or why not. Read before anything
/// is typed; an unreadable report is a no. It can only stop a fill, never let one through: the
/// extension's report can lag a navigation by a moment, so each value goes only after box-exec's
/// own snap of the page says it's on the site ([`snap`]), and its type carries `allow_domains`
/// (`a_page_off_the_cards_site_types_nothing_and_the_card_stays_open` holds the extension agreeing
/// while the page is elsewhere).
async fn front_page(live: &Live, site: &str) -> Result<(), String> {
    let front = live.provider.active_tab(&live.machine, &live.screen).await.map_err(|_| "The page in front couldn't be read, so nothing was typed.".to_owned())?;
    match front.url {
        None => Err("No page is open in the browser on this Bot's screen, so nothing was typed. Open the sign-in page first.".to_owned()),
        Some(url) if on_site(&url, site) => Ok(()),
        Some(url) => Err(format!("The page in front is {}, not {site}, so nothing was typed.", page_host(&url).unwrap_or_else(|| "not a web page".to_owned()))),
    }
}

/// Types `values` into the page on `live`'s screen by OpenGrok's rules (see the module's words).
/// No picture is taken. `values` are by field id, this card's alone.
pub async fn fill(live: &Live, form: &Form, values: &BTreeMap<String, Secret>, typing: &AtomicBool) -> Fill {
    let to_type: Vec<&Field> = form.fields.iter().take(if form.types_only_first_field() { 1 } else { form.fields.len() }).filter(|f| values.get(&f.id).is_some_and(|v| !v.expose().is_empty())).collect();
    if to_type.is_empty() {
        return Fill::Empty;
    }
    if let Err(message) = browser_layer(live).await {
        return Fill::Failed { outcomes: Vec::new(), reason: message, uncertain: false, maybe_secret: false };
    }
    if let Err(message) = front_page(live, &form.site).await {
        return Fill::Refused { code: "wrong-page", message };
    }
    let mut outcomes: Vec<Outcome> = form.fields.iter().map(|f| Outcome { id: f.id.clone(), filled: false, sensitive: false }).collect();
    let mut taken: Vec<String> = Vec::new();
    let mut last: Option<&Field> = None;
    for (i, field) in to_type.iter().enumerate() {
        let value = &values[&field.id];
        let mut tries = 0;
        let typed = loop {
            tries += 1;
            let (snapshot, elements) = match snap(live, &form.site).await {
                Ok(s) => s,
                // nothing typed yet: the card stays open, as for the front page
                Err(SnapErr::OffSite(host)) if i == 0 => return Fill::Refused { code: "wrong-page", message: format!("The page in front is {host}, not {}, so nothing was typed.", form.site) },
                Err(e) => break Err((e.sentence(&form.site), false)),
            };
            let r = match pick(&elements, field, &taken) {
                Ok(r) => r,
                Err(reason) => break Err((reason, false)),
            };
            let body = json!({"ref": r, "snapshot": snapshot, "action": "type", "text": value.expose(), "allow_domains": allow(&form.site)});
            // the type went out: a broken answer doesn't say it didn't land
            // from here a value may be in the page, whatever becomes of this run
            typing.store(true, Ordering::SeqCst);
            let reply = match live.provider.browser(&live.machine, &live.screen, BrowserOp::Act, &body).await {
                Ok(r) => r,
                Err(p) => break Err((p.sentence(), true)),
            };
            drop(body);
            if reply.ok() {
                let el = elements.iter().find(|e| e["ref"] == r.as_str());
                let label = el.map(|e| norm(e["label"].as_str().unwrap_or_default())).unwrap_or_default();
                break Ok((label, el.is_some_and(|e| e["sensitive"].as_bool().unwrap_or(false))));
            }
            // a page that changed under the snap: once more, from a fresh one
            if reply.code() == Some("stale_ref") && tries == 1 {
                continue;
            }
            break Err((refusal_sentence(&reply, &form.site), uncertain(&reply)));
        };
        match typed {
            Ok((label, sensitive)) => {
                log(field_log_line(field, sensitive, value.expose().chars().count(), "typed"));
                taken.push(label);
                if let Some(o) = outcomes.iter_mut().find(|o| o.id == field.id) {
                    o.filled = true;
                    o.sensitive = sensitive;
                }
                last = Some(field);
            }
            Err((reason, uncertain)) => {
                log(field_log_line(field, false, value.expose().chars().count(), if uncertain { "may have been typed" } else { "not typed" }));
                // the value whose type may have landed was a secret: the page may hold it
                let maybe_secret = uncertain && field.is_secret();
                return Fill::Failed { outcomes, reason, uncertain, maybe_secret };
            }
        }
    }
    let typed = outcomes.iter().filter(|o| o.filled).count();
    let pressed = match last.filter(|_| should_press_return(form, typed)) {
        Some(field) => press_return(live, form, field, &taken[..taken.len().saturating_sub(1)]).await,
        None => Pressed::No,
    };
    Fill::Filled { outcomes, pressed }
}

/// Return in the field typed last, from a fresh snap of the page on the card's site. A press the
/// box asks a person to confirm stays unpressed.
async fn press_return(live: &Live, form: &Form, field: &Field, others: &[String]) -> Pressed {
    let (snapshot, elements) = match snap(live, &form.site).await {
        Ok(s) => s,
        Err(e) => return Pressed::Failed(e.sentence(&form.site)),
    };
    let r = match pick(&elements, field, others) {
        Ok(r) => r,
        Err(reason) => return Pressed::Failed(reason),
    };
    let body = json!({"ref": r, "snapshot": snapshot, "action": "key", "text": "Enter", "allow_domains": allow(&form.site)});
    match live.provider.browser(&live.machine, &live.screen, BrowserOp::Act, &body).await {
        Ok(reply) if reply.ok() => Pressed::Yes,
        Ok(reply) if reply.code() == Some("confirm_required") => Pressed::NeedsConfirm(reply.body.pointer("/error/details/label").and_then(Value::as_str).map(|l| js::slice_utf16(l, 80).to_owned()).unwrap_or_else(|| "the form's button".to_owned())),
        Ok(reply) if uncertain(&reply) => Pressed::Uncertain(refusal_sentence(&reply, &form.site)),
        Ok(reply) => Pressed::Failed(refusal_sentence(&reply, &form.site)),
        // the Enter went out: a broken answer doesn't say it wasn't pressed
        Err(p) => Pressed::Uncertain(p.sentence()),
    }
}

// ---- what the model reads ----

const SECRET_NOTE: &str = "Secret field values were typed into the page and never shown to you.";
/// A failed fill whose last type, a secret's, may have landed.
pub const MAYBE_SECRET_NOTE: &str = "A secret value may have been typed into the page; it was never shown to you.";

/// Whether a failed fill put a secret into the page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SecretTyped {
    Yes,
    /// the type of one went out and its answer didn't say how it went
    Maybe,
    No,
}

/// [`SecretTyped`] of a failed fill: a secret field filled, else `maybe_secret`.
pub fn secret_typed(form: &Form, outcomes: &[Outcome], maybe_secret: bool) -> SecretTyped {
    match () {
        _ if form.fields.iter().filter(|f| f.is_secret()).any(|f| outcomes.iter().any(|o| o.id == f.id && o.filled)) => SecretTyped::Yes,
        _ if maybe_secret => SecretTyped::Maybe,
        _ => SecretTyped::No,
    }
}
const OBSERVE: &str = "Take a screenshot and confirm what the page shows now; do not claim login succeeded until you see it. If another challenge appears on the page (a one-time code), call request_user_form again with otp fields; never raise a card that already settled again. A captcha, or a prompt outside the page, is for computer_ask_person.";
const ONLY_FIRST: &str = "Only the FIRST field was typed: this card had several fields but no samePage. Take a screenshot and confirm what the page shows now; do not claim login succeeded. If the rest are on the same page, raise ONE new card for them with samePage true; if a password page comes next, raise a password-only card (challengeKind \"password\"). Never raise a card that already settled again.";
pub const DISMISSED: &str = "without filling anything. Continue without those credentials; do not type secrets with computer_type, and do not raise the same card again.";
pub const TIMED_OUT: &str = "The wait timed out. Continue without those credentials; do not type secrets with computer_type, and do not raise the same card again.";
pub const HANDED_BACK: &str = "The person finished on your screen and handed it back. Take a screenshot and confirm what the page shows; do not claim login succeeded until you see it. If another challenge is on the page, call request_user_form.";
pub const HANDOVER_TIMED_OUT: &str = "The person chose to finish on your screen, and their time ran out, so they may not have finished. Take a screenshot and see where the page is; do not claim login succeeded until you see it.";

/// What the model is told of a fill. Only fields that aren't secret are named with their values
/// (`shared`); a secret field isn't named at all.
pub fn result_text(form: &Form, fill: &Fill, shared: &BTreeMap<String, String>) -> String {
    let title = &form.title;
    let observe = if form.types_only_first_field() { ONLY_FIRST } else { OBSERVE };
    match fill {
        Fill::Filled { pressed, .. } => {
            let pressed = match pressed {
                Pressed::Yes => "Return was pressed in the last field.".to_owned(),
                Pressed::No => "Nothing was pressed: press the page's own button after you look.".to_owned(),
                Pressed::NeedsConfirm(label) => format!("Return wasn't pressed: the page asks a person to confirm \"{label}\" first. Ask the person to press it themselves; don't press it for them."),
                Pressed::Failed(why) => format!("Return wasn't pressed: {why}"),
                Pressed::Uncertain(why) => format!("{why} {MAYBE_PRESSED}"),
            };
            let shared = if shared.is_empty() { "No non-secret fields were shared.".to_owned() } else { format!("Shared fields: {}.", shared.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", ")) };
            format!("The person submitted the form \"{title}\". It was filled into the page. {pressed} {observe} {shared} {SECRET_NOTE}")
        }
        Fill::Failed { outcomes, reason, uncertain, maybe_secret } => {
            let maybe = if *uncertain { format!(" {MAYBE_TYPED}") } else { String::new() };
            // a secret that went in, one that may have, or none: "nothing more was typed" stays true
            let secret = match secret_typed(form, outcomes, *maybe_secret) {
                SecretTyped::Yes => format!("{SECRET_NOTE} "),
                SecretTyped::Maybe => format!("{MAYBE_SECRET_NOTE} "),
                SecretTyped::No => String::new(),
            };
            format!("The person submitted the form \"{title}\" but it could not be filled into the page: {reason}{maybe} {observe} {secret}Do not type secrets with computer_type.")
        }
        Fill::Refused { message, .. } => format!("The form \"{title}\" wasn't filled: {message}"),
        Fill::Empty => format!("The person submitted the form \"{title}\" with every field empty, so nothing was typed into the page. {EMPTY_NEXT}"),
    }
}

/// The values the model may read: those of fields that aren't secret by their own words, and that
/// went into an element the box doesn't call sensitive.
pub fn shared_values(form: &Form, values: &BTreeMap<String, Secret>, outcomes: &[Outcome]) -> BTreeMap<String, String> {
    form.fields
        .iter()
        .filter(|f| !f.is_secret() && outcomes.iter().any(|o| o.id == f.id && o.filled && !o.sensitive))
        .filter_map(|f| values.get(&f.id).map(|v| (f.id.clone(), js::slice_utf16(v.expose(), 200).to_owned())))
        .collect()
}

// ---- the card and its answers ----

/// What a submit or a dismiss asks of the run waiting on a card.
pub enum Ask {
    Submit { values: BTreeMap<String, Secret>, saved: bool, reply: oneshot::Sender<Answer> },
    Dismiss { escalate: bool, reply: oneshot::Sender<Answer> },
}

/// The run's answer to the route: `Ok` the settled notice (and for a typed login that went in,
/// what the app may offer to save), `Err` a refusal with the card still open.
pub type Answer = Result<Value, (u16, &'static str, String)>;

type Waiting = HashMap<String, mpsc::Sender<Ask>>;

/// The cards a run waits on, by id.
fn waiting() -> MutexGuard<'static, Waiting> {
    static W: OnceLock<Mutex<Waiting>> = OnceLock::new();
    W.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Every `user_form` notice in `chat` with id `id` gets `f`.
fn edit(ctx: &HubCtx, chat: &str, id: &str, f: impl Fn(&mut Value)) -> Option<Value> {
    let found = ctx
        .store
        .write(|s| {
            let t = s.thread_mut(chat)?;
            let n = t.turns.iter_mut().filter_map(|u| u.notice.as_mut()).find(|n| n["kind"] == KIND && n["id"] == id)?;
            f(n);
            Some(n.clone())
        })
        .ok()
        .flatten();
    if found.is_some() {
        ctx.store.write_index();
    }
    found
}

pub fn find(ctx: &HubCtx, chat: &str, id: &str) -> Option<Value> {
    ctx.store.read(|s| s.thread(chat).and_then(|t| t.turns.iter().filter_map(|u| u.notice.as_ref()).find(|n| n["kind"] == KIND && n["id"] == id).cloned()))
}

fn settle(ctx: &HubCtx, chat: &str, id: &str, state: &str, extra: Value) -> Option<Value> {
    let at = crate::hub::policy::now(ctx);
    edit(ctx, chat, id, |n| {
        n["state"] = json!(state);
        n["at"] = json!(at);
        if let Some(m) = extra.as_object() {
            for (k, v) in m {
                n[k] = v.clone();
            }
        }
    })
}

/// A card's place in [`waiting`]: when the run stops waiting, however, it goes, and a card still
/// pending expires.
struct Parked {
    ctx: Arc<HubCtx>,
    chat: String,
    id: String,
    /// a fill sent a value to the box: a run stopped after that may have left it in the page
    typing: Arc<AtomicBool>,
    /// the card has a secret field: which value was in flight isn't known, so it may be one
    secret: bool,
}

impl Drop for Parked {
    fn drop(&mut self) {
        waiting().remove(&self.id);
        if find(&self.ctx, &self.chat, &self.id).is_some_and(|n| n["state"] == "pending") {
            if self.typing.load(Ordering::SeqCst) {
                settle(&self.ctx, &self.chat, &self.id, "fill_failed", json!({"uncertain": true, "secretMaybe": self.secret, "note": format!("The run stopped part way through the fill. {MAYBE_TYPED}")}));
            } else {
                settle(&self.ctx, &self.chat, &self.id, "expired", Value::Null);
            }
        }
    }
}

fn outcomes_json(outcomes: &[Outcome]) -> Value {
    json!(outcomes.iter().map(|o| json!({"id": o.id, "filled": o.filled})).collect::<Vec<_>>())
}

/// `request_user_form`, on a screen this run holds: posts the card and waits for its answer.
pub async fn request(run: &Run, computers: &Arc<Computers>, live: Live, input: &Value) -> Out {
    let form = match sanitize(input) {
        Ok(f) => f,
        Err(why) => return Out::err(why),
    };
    if live.machine.link.is_some() {
        return Out::err("A remote computer's browser can't be typed into from here. Ask the person in chat to sign in on that machine themselves.".to_owned());
    }
    if !asking::present(&run.turn) || !run.ctx.ui.connected() {
        return Out::err("Nobody is here to fill a sign-in card now. Say in chat that you need them to sign in, and stop there.".to_owned());
    }
    let ctx = &run.ctx;
    let chat = run.thread_id();
    let id = format!("uf_{}", crate::hub::store::uuidv7());
    let n = json!({
        "kind": KIND, "id": id, "bot": live.bot, "botName": crate::hub::computer::bot_name(ctx, &live.bot), "chat": chat,
        "title": form.title, "form": form.to_json(), "takeoverNote": TAKEOVER_NOTE, "returnPress": return_press(&form), "state": "pending", "at": crate::hub::policy::now(ctx),
    });
    let (tx, mut rx) = mpsc::channel::<Ask>(4);
    waiting().insert(id.clone(), tx);
    let typing = Arc::new(AtomicBool::new(false));
    let parked = Parked { ctx: ctx.clone(), chat: chat.clone(), id: id.clone(), typing: typing.clone(), secret: form.has_secret() };
    if !crate::hub::tools::miniapps::post_in(ctx, &chat, &n) {
        return Out::err("The card couldn't be put in this chat, so nothing was asked.".to_owned());
    }
    run.set_job("waiting", asking::WAITING);
    let hold = ctx.ext.get::<LoginWait>().map_or(HOLD, |w| w.0);
    let deadline = tokio::time::Instant::now() + hold;
    let out = loop {
        let ask = match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(ask)) => ask,
            Ok(None) | Err(_) => {
                settle(ctx, &chat, &id, "timed_out", Value::Null);
                break Out::ok(TIMED_OUT.to_owned());
            }
        };
        match ask {
            Ask::Submit { values, saved, reply } => match submitted(run, computers, &form, &chat, &id, values, saved, &typing).await {
                Ok((answer, out)) => {
                    let _ = reply.send(Ok(answer));
                    break out;
                }
                Err(refusal) => {
                    let _ = reply.send(Err(refusal));
                }
            },
            Ask::Dismiss { escalate: false, reply } => {
                let n = settle(ctx, &chat, &id, "dismissed", Value::Null).unwrap_or(Value::Null);
                let _ = reply.send(Ok(json!({"ok": true, "notice": n})));
                break Out::ok(format!("The person dismissed the form \"{}\" {DISMISSED}", form.title));
            }
            Ask::Dismiss { escalate: true, reply } => match escalated(run, computers, &chat, &id, &form, reply).await {
                Some(out) => break out,
                None => continue,
            },
        }
    };
    drop(parked);
    run.set_job("connecting", "Working on its computer");
    out
}

/// A submit: the fill, the card settled, and what the model reads. `Err` leaves the card open.
#[allow(clippy::too_many_arguments)]
async fn submitted(run: &Run, computers: &Arc<Computers>, form: &Form, chat: &str, id: &str, values: BTreeMap<String, Secret>, saved: bool, typing: &AtomicBool) -> Result<(Value, Out), (u16, &'static str, String)> {
    let ctx = &run.ctx;
    let values: BTreeMap<String, Secret> = values.into_iter().filter(|(k, _)| form.fields.iter().any(|f| &f.id == k)).collect();
    for field in &form.fields {
        let len = values.get(&field.id).map_or(0, |v| v.expose().chars().count());
        if field.required && len == 0 {
            return Err((400, "invalid_args", format!("\"{}\" is needed.", field.label)));
        }
        log(field_log_line(field, false, len, "received; value not logged"));
    }
    let live = computers.live(ctx, &bot_of(ctx, chat, id)).await.map_err(|p| (409, "unavailable", p.sentence()))?;
    if computers.person_on(&live.bot) {
        return Err((409, "person_on", "A person has this Bot's screen right now, so nothing was typed. Try again once they hand it back.".to_owned()));
    }
    if saved {
        let seats = Computers::seats(ctx, &live.computer).map_err(|p| (500, "internal", p.sentence()))?;
        if seats.len() > 1 {
            return Err((403, SHARED_COMPUTER, SHARED_COMPUTER_MESSAGE.to_owned()));
        }
    }
    run.set_job("connecting", "Typing the sign-in into the page");
    let fill = fill(&live, form, &values, typing).await;
    Computers::touch(ctx, &live.computer);
    if let Fill::Refused { code, message } = fill {
        run.set_job("waiting", asking::WAITING);
        return Err((403, code, message));
    }
    let (state, outcomes, extra) = match &fill {
        Fill::Filled { outcomes, pressed } => {
            // the card says what happened, step by step
            let note = match pressed {
                _ if form.types_only_first_field() => format!("Typed the first step ({}); the rest is for the page that comes next.", form.fields[0].label),
                Pressed::NeedsConfirm(label) => format!("Typed. The page asks you to confirm \"{label}\": press it yourself on the screen."),
                Pressed::Failed(why) => format!("Typed, but Return wasn't pressed: {why}"),
                Pressed::Uncertain(why) => format!("Typed. {why} {MAYBE_PRESSED}"),
                Pressed::No => "Typed. Return is left to you, or to the AI once it has looked.".to_owned(),
                Pressed::Yes => "Typed, and Return was pressed.".to_owned(),
            };
            ("submitted", outcomes.clone(), json!({"pressed": matches!(pressed, Pressed::Yes), "note": note}))
        }
        Fill::Failed { outcomes, reason, uncertain, maybe_secret } => {
            let note = if *uncertain { format!("{reason} {MAYBE_TYPED}") } else { reason.clone() };
            // the card says so when a secret may be in the page
            let secret_maybe = secret_typed(form, outcomes, *maybe_secret) == SecretTyped::Maybe;
            ("fill_failed", outcomes.clone(), json!({"note": note, "uncertain": uncertain, "secretMaybe": secret_maybe}))
        }
        Fill::Empty => ("fill_failed", form.fields.iter().map(|f| Outcome { id: f.id.clone(), filled: false, sensitive: false }).collect(), json!({"note": EMPTY_NOTE})),
        Fill::Refused { .. } => unreachable!("handled above"),
    };
    let mut extra = extra;
    extra["outcomes"] = outcomes_json(&outcomes);
    let n = settle(ctx, chat, id, state, extra).unwrap_or(Value::Null);
    let shared = shared_values(form, &values, &outcomes);
    let mut answer = json!({"ok": true, "notice": n});
    // a typed login that went in may be saved on the Mac: the app has the password, this sends
    // only where and as whom
    // only a login typed in full, every secret of it, and not one whose Return may have gone twice
    let whole = matches!(&fill, Fill::Filled { pressed, .. } if !matches!(pressed, Pressed::Uncertain(_)))
        && form.fields.iter().filter(|f| f.is_secret()).all(|f| outcomes.iter().any(|o| o.id == f.id && o.filled));
    if !saved && whole && form.has_secret() {
        let username = form.fields.iter().filter(|f| !f.is_secret()).find_map(|f| shared.get(&f.id)).cloned();
        answer["offerSave"] = json!({"origin": form.site, "username": username});
    }
    let out = Out::ok(result_text(form, &fill, &shared));
    drop(values);
    Ok((answer, out))
}

/// "Open the screen": the screen is the person's, as a hand-over is, and then the card settles
/// escalated. `None` when it can't be handed over now: the card stays open, and says why.
async fn escalated(run: &Run, computers: &Arc<Computers>, chat: &str, id: &str, form: &Form, reply: oneshot::Sender<Answer>) -> Option<Out> {
    let ctx = &run.ctx;
    let live = match computers.live(ctx, &bot_of(ctx, chat, id)).await {
        Ok(l) => l,
        Err(p) => {
            let _ = reply.send(Err((409, "unavailable", p.sentence())));
            return None;
        }
    };
    if !matches!(live.provider.users_apart(&live.machine, std::slice::from_ref(&live.bot)).await, Ok(true)) {
        let _ = reply.send(Err((409, "not_apart", "This computer's commands can't be confirmed walled off from its screen right now, so the screen wasn't handed over.".to_owned())));
        return None;
    }
    // the screen is the person's first: only then does the card say so
    let reason = format!("Sign in: {}", form.title);
    let handed = match tools::begin_handover(run, computers, live, &reason, Duration::from_secs(HANDOVER_S)).await {
        Ok(h) => h,
        Err(out) => {
            let why = out.content.as_str().map(str::to_owned).unwrap_or_else(|| "The screen couldn't be handed over now.".to_owned());
            let _ = reply.send(Err((409, "not_handed_over", format!("The screen wasn't handed over: {why}"))));
            return None;
        }
    };
    let n = settle(ctx, chat, id, "escalated", Value::Null).unwrap_or(Value::Null);
    let _ = reply.send(Ok(json!({"ok": true, "notice": n})));
    let out = match tools::finish_handover(run, computers, handed).await {
        Ok((_, facts)) if facts["outcome"] == "handed_back" => Out::ok(HANDED_BACK.to_owned()),
        Ok(_) => Out::ok(HANDOVER_TIMED_OUT.to_owned()),
        Err(out) => out,
    };
    Some(out)
}

fn bot_of(ctx: &HubCtx, chat: &str, id: &str) -> String {
    find(ctx, chat, id).and_then(|n| n["bot"].as_str().map(str::to_owned)).unwrap_or_default()
}

pub const SHARED_COMPUTER: &str = "shared-computer";
pub const SHARED_COMPUTER_MESSAGE: &str = "This computer is shared with other bots or people, so a saved login is not used on it. Type the login by hand, or give this bot its own computer.";

fn refused(status: u16, code: &str, message: impl Into<String>) -> (u16, Value) {
    (status, json!({"ok": false, "code": code, "message": message.into(), "retryable": false}))
}

/// The card `chat`/`id` the body names, pending, and the run waiting on it.
fn waiting_card(ctx: &HubCtx, b: &Value) -> Result<(String, String, mpsc::Sender<Ask>), (u16, Value)> {
    let str_of = |k: &str| b.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_owned);
    let (Some(chat), Some(id)) = (str_of("chat"), str_of("id")) else { return Err(refused(400, "invalid_args", "send {chat, id, …}")) };
    let Some(n) = find(ctx, &chat, &id) else { return Err(refused(404, "card_missing", "There's no such card in this chat.")) };
    if n["state"] != "pending" {
        return Err((409, json!({"ok": false, "code": "stale", "message": "This card was answered already.", "retryable": false, "notice": n})));
    }
    let Some(tx) = waiting().get(&id).cloned() else {
        let n = settle(ctx, &chat, &id, "expired", Value::Null).unwrap_or(n);
        return Err((409, json!({"ok": false, "code": "stale", "message": "Nothing waits on this card any more, so nothing was typed.", "retryable": false, "notice": n})));
    };
    Ok((chat, id, tx))
}

/// Hands an answer to the run and waits for what it made of it. A reply that can't say (the run
/// went on without it, or it's still typing) carries the card as it stands now, so the app paints
/// what really happened, never "nothing was filled" on a card that was.
async fn put(ctx: &HubCtx, chat: &str, id: &str, tx: mpsc::Sender<Ask>, make: impl FnOnce(oneshot::Sender<Answer>) -> Ask) -> (u16, Value) {
    let as_it_stands = |status: u16, code: &str, message: &str| {
        let (status, mut v) = refused(status, code, message);
        if let Some(n) = find(ctx, chat, id) {
            v["notice"] = n;
        }
        (status, v)
    };
    let (reply, answer) = oneshot::channel();
    if tx.send(make(reply)).await.is_err() {
        return as_it_stands(409, "stale", "This card was answered already.");
    }
    let patience = ctx.ext.get::<AnswerWait>().map_or(ANSWER_PATIENCE, |w| w.0);
    match tokio::time::timeout(patience, answer).await {
        Ok(Ok(Ok(v))) => (200, v),
        Ok(Ok(Err((status, code, message)))) => refused(status, code, message),
        Ok(Err(_)) => as_it_stands(409, "stale", "This card was answered already."),
        Err(_) => as_it_stands(504, "timeout", "It's still being typed, or the fill took too long to say how it went. Look at the Bot's screen before doing anything else."),
    }
}

/// `POST /v1/host/computer/login/submit {chat, id, values: {<field id>: text}, savedLogin?}`.
/// Human only (the host token). The values go to the run that waits on the card and nowhere else;
/// nothing of them is logged or answered.
pub async fn host_submit(ctx: &Arc<HubCtx>, b: &Value) -> (u16, Value) {
    let (chat, id, tx) = match waiting_card(ctx, b) {
        Ok(found) => found,
        Err(e) => return e,
    };
    let Some(raw) = b.get("values").and_then(Value::as_object) else { return refused(400, "invalid_args", "values is an object of field id to text") };
    let mut values = BTreeMap::new();
    for (k, v) in raw {
        let Some(t) = v.as_str() else { return refused(400, "invalid_args", "each value is text") };
        if t.chars().count() > MAX_VALUE {
            return refused(400, "invalid_args", "a value is too long");
        }
        values.insert(k.clone(), Secret::new(t));
    }
    let saved = b.get("savedLogin").and_then(Value::as_bool).unwrap_or(false);
    put(ctx, &chat, &id, tx, |reply| Ask::Submit { values, saved, reply }).await
}

/// `POST /v1/host/computer/login/dismiss {chat, id, mode: "dismissed" | "escalated"}`.
pub async fn host_dismiss(ctx: &Arc<HubCtx>, b: &Value) -> (u16, Value) {
    let escalate = match b.get("mode").and_then(Value::as_str) {
        Some("dismissed") | None => false,
        Some("escalated") => true,
        Some(_) => return refused(400, "invalid_args", "mode is \"dismissed\" or \"escalated\""),
    };
    let (chat, id, tx) = match waiting_card(ctx, b) {
        Ok(found) => found,
        Err(e) => return e,
    };
    put(ctx, &chat, &id, tx, |reply| Ask::Dismiss { escalate, reply }).await
}

/// `POST /v1/host/computer/login/saved {chat, id}`: whether a saved login may be used on this
/// card, asked before Touch ID so a refusal never follows a fingerprint. `{ok, usable, code?,
/// message?}`. This answers for the computer only (the Bot sits alone on it); whether the login is
/// shared with the Bot is the app's to decide, and it has by the time it asks.
pub fn host_saved(ctx: &Arc<HubCtx>, b: &Value) -> (u16, Value) {
    let (chat, id, _) = match waiting_card(ctx, b) {
        Ok(found) => found,
        Err(e) => return e,
    };
    let bot = bot_of(ctx, &chat, &id);
    let seat = match Computers::seat(ctx, &bot) {
        Ok(Some(seat)) => seat,
        Ok(None) => return (200, json!({"ok": true, "usable": false, "code": "no_computer", "message": "This Bot has no computer now."})),
        Err(p) => return refused(500, "internal", p.sentence()),
    };
    match Computers::seats(ctx, &seat.computer) {
        Ok(seats) if seats.len() > 1 => (200, json!({"ok": true, "usable": false, "code": SHARED_COMPUTER, "message": SHARED_COMPUTER_MESSAGE})),
        Ok(_) => (200, json!({"ok": true, "usable": true})),
        Err(p) => refused(500, "internal", p.sentence()),
    }
}

/// `POST /v1/host/computer/bot/{bot}/session {op: "save" | "load", name}`: a person's
/// `/computer session save|load <name>`, the Bot's own screen's cookies and storage kept in the
/// box (`/v1/browser/state`). Counts come back, never values.
pub async fn session(ctx: &HubCtx, computers: &Computers, bot: &str, b: &Value) -> (u16, Value) {
    let op = match b.get("op").and_then(Value::as_str) {
        Some(op @ ("save" | "load")) => op,
        _ => return refused(400, "invalid_args", "op is \"save\" or \"load\""),
    };
    let name = b.get("name").and_then(Value::as_str).unwrap_or_default();
    if name.is_empty() || name.len() > 64 || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-') {
        return refused(400, "invalid_args", "A session's name is 1 to 64 letters, digits, _ and -.");
    }
    let live = match computers.live(ctx, bot).await {
        Ok(l) => l,
        Err(p) => return refused(409, p.code(), p.sentence()),
    };
    if live.machine.link.is_some() {
        return refused(409, "unsupported", "A remote computer's browser sessions aren't kept from here.");
    }
    if let Err(why) = browser_layer(&live).await {
        return refused(409, "unsupported", why);
    }
    match live.provider.browser(&live.machine, &live.screen, BrowserOp::State, &json!({"op": op, "name": name})).await {
        Ok(reply) if reply.ok() => {
            let count = |k: &str| reply.body.get(k).and_then(Value::as_array).map_or(0, Vec::len);
            (200, json!({"ok": true, "op": op, "name": name, "cookies": reply.body["cookies"].as_u64().unwrap_or(0), "origins": count("origins"), "pending": count("pending_origins")}))
        }
        Ok(reply) if reply.code() == Some("not_found") => refused(404, "not_found", format!("There's no session called {name} on this Bot's computer.")),
        Ok(reply) => refused(409, "refused", refusal_sentence(&reply, "this page")),
        Err(p) => refused(409, p.code(), p.sentence()),
    }
}
